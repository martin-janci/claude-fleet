<script lang="ts">
  // Settings › Organisations › Debug devices: the device board
  // (`docs/debug-devices.md`). Every phone, emulator and simulator the
  // fleet's hosts see, grouped by host as cards: whether it is ready, who
  // holds it and who waits, a screenshot, and the actions a person takes by
  // hand (claim, wait in line, release, mirror, logs, start / stop). The
  // generated page below it stays the full editor (label, sharing, install,
  // forget). On a paired desktop the same commands route to the hub's
  // `debug_devices`, so nothing here branches on the mode.
  //
  // Mounted only while its leaf is open (SettingsDialog), so the 30 s
  // refresh runs only while the board is on screen, and not while the
  // window is hidden.
  import { onDestroy, onMount } from 'svelte';
  import Button from './kit/Button.svelte';
  import Icon from './kit/Icon.svelte';
  import StatusChip from './kit/StatusChip.svelte';
  import Loader from './Loader.svelte';
  import Modal from './Modal.svelte';
  import type { IpcError, Result } from './result';
  import { timeAgo } from './session_status';
  import {
    bootDebugDevice,
    claimDebugDevice,
    debugDeviceLogs,
    debugDeviceScreenshot,
    deviceSubtitle,
    holderName,
    listDebugDevices,
    placeName,
    releaseDebugDevice,
    scanDebugDevices,
    shutdownDebugDevice,
    startDeviceMirror,
    stateHint,
    stateLabel,
    stopDeviceMirror,
    DESKTOP_HOLDER,
    type DebugDevice,
    type RunOutput,
    type Screenshot,
  } from './debug_devices';
  import {
    PLATFORM_FILTERS,
    STATE_FILTERS,
    cardActions,
    claimLine,
    countLine,
    filterDevices,
    groupByHost,
    screenshotSrc,
    stateTone,
    waitingOf,
    type CardAction,
    type PlatformFilter,
    type StateFilter,
  } from './debug_devices_board';

  /** The page spec's `empty` (resources.rs, `debug_device`), word for word. */
  const EMPTY =
    'No debug devices yet. Plug a phone into a host (USB debugging on), start an emulator or boot a simulator; hosts are scanned every few minutes, or rescan one now.';
  const REFRESH_MS = 30_000;

  let devices = $state<DebugDevice[] | null>(null);
  let loadError = $state<IpcError | null>(null);
  let loading = $state(false);
  let rescanning = $state(false);
  let scanError = $state<string | null>(null);
  let loadedAt = $state<number | null>(null);
  let nowMs = $state(Date.now());

  let stateFilter = $state<StateFilter>('all');
  let platform = $state<PlatformFilter>('any');
  let query = $state('');

  /** Per card: the action in flight, its last error, the note typed. */
  let busy = $state<Record<number, CardAction | null>>({});
  let errors = $state<Record<number, string>>({});
  let notes = $state<Record<number, string>>({});
  let linesOpen = $state<Record<number, boolean>>({});
  /** A destructive action waiting for its yes. */
  let confirming = $state<{ id: number; action: 'release' | 'shutdown' } | null>(null);
  /** Screenshots, per device id, for the board's lifetime. */
  let shots = $state<Record<number, Screenshot>>({});
  let enlarged = $state<number | null>(null);
  /** Mirrors this board started, and what each start said. */
  let mirroring = $state<Record<number, boolean>>({});
  let mirrorOut = $state<Record<number, string>>({});
  let logs = $state<{ title: string; out: RunOutput } | null>(null);

  const nowSecs = $derived(Math.floor(nowMs / 1000));
  const shown = $derived(devices ? filterDevices(devices, { state: stateFilter, platform, query }) : []);
  const groups = $derived(groupByHost(shown));
  const byId = (id: number) => devices?.find((d) => d.id === id);

  async function load() {
    loading = true;
    const r = await listDebugDevices();
    loading = false;
    nowMs = Date.now();
    if (r.ok) {
      devices = r.value;
      loadError = null;
      loadedAt = nowMs;
    } else {
      loadError = r.error;
    }
  }

  async function rescan() {
    rescanning = true;
    scanError = null;
    const r = await scanDebugDevices();
    if (!r.ok) scanError = r.error.message;
    await load();
    rescanning = false;
  }

  let timer: ReturnType<typeof setInterval> | undefined;
  onMount(() => {
    void load();
    timer = setInterval(() => {
      nowMs = Date.now();
      if (typeof document !== 'undefined' && document.hidden) return;
      // A card mid-action keeps its own answer; the next tick catches up.
      if (loading || rescanning || Object.values(busy).some(Boolean)) return;
      void load();
    }, REFRESH_MS);
  });
  onDestroy(() => clearInterval(timer));

  /** Run one card action: busy while it runs, its error on the card, and
   *  the device it answers (when it answers one) in place of the old row. */
  async function act<T>(id: number, action: CardAction, run: () => Promise<Result<T>>): Promise<T | null> {
    busy[id] = action;
    delete errors[id];
    confirming = null;
    const r = await run();
    busy[id] = null;
    if (!r.ok) {
      errors[id] = r.error.message;
      return null;
    }
    const v = r.value as unknown;
    if (devices && v && typeof v === 'object' && typeof (v as DebugDevice).id === 'number' && 'state' in v) {
      devices = devices.map((d) => (d.id === id ? (v as DebugDevice) : d));
    }
    return r.value;
  }

  const noteOf = (id: number) => notes[id]?.trim() || undefined;

  async function claim(d: DebugDevice, queue: boolean) {
    const done = await act(d.id, queue ? 'queue' : 'claim', () =>
      claimDebugDevice(d.id, { note: noteOf(d.id), queue }),
    );
    if (done) delete notes[d.id];
  }

  /** Leaving the line and releasing are the same command: from inside the
   *  line it only leaves it. */
  const release = (d: DebugDevice, as: 'release' | 'leave') => act(d.id, as, () => releaseDebugDevice(d.id));

  async function screenshot(d: DebugDevice) {
    const s = await act(d.id, 'screenshot', () => debugDeviceScreenshot(d.id));
    if (s) shots[d.id] = s;
  }

  async function mirror(d: DebugDevice) {
    const m = await act(d.id, 'mirror', () => startDeviceMirror(d.id));
    if (!m) return;
    mirroring[d.id] = true;
    mirrorOut[d.id] = m.output.trim() || `Mirroring ${m.title} in a scrcpy window.`;
    void load();
  }

  async function stopMirror(d: DebugDevice) {
    await act(d.id, 'stop-mirror', () => stopDeviceMirror(d.id));
    // Stopped, or it had already ended (`E_NOTFOUND`, shown on the card):
    // either way the board stops offering Stop mirror for it, unless the
    // reloaded row still carries the mirror's claim.
    delete mirroring[d.id];
    delete mirrorOut[d.id];
    void load();
  }

  async function openLogs(d: DebugDevice) {
    const out = await act(d.id, 'logs', () => debugDeviceLogs(d.id));
    if (out) logs = { title: d.title, out };
  }

  async function boot(d: DebugDevice) {
    if ((await act(d.id, 'boot', () => bootDebugDevice(d.id))) !== null) void load();
  }

  const shutdown = (d: DebugDevice) => act(d.id, 'shutdown', () => shutdownDebugDevice(d.id));

  const ACTION_LABEL: Record<CardAction, [string, string]> = {
    claim: ['Claim', 'Claiming…'],
    queue: ['Wait in line', 'Joining…'],
    leave: ['Leave line', 'Leaving…'],
    release: ['Release', 'Releasing…'],
    screenshot: ['Screenshot', 'Capturing…'],
    mirror: ['Mirror', 'Starting…'],
    'stop-mirror': ['Stop mirror', 'Stopping…'],
    logs: ['Logs…', 'Reading…'],
    boot: ['Start', 'Starting…'],
    shutdown: ['Stop', 'Stopping…'],
  };

  function run(d: DebugDevice, a: CardAction) {
    switch (a) {
      case 'claim':
        return claim(d, false);
      case 'queue':
        return claim(d, true);
      case 'leave':
        return release(d, 'leave');
      case 'release':
        // Your own claim goes at once; someone else's asks first.
        if (d.claimed_by === DESKTOP_HOLDER) return release(d, 'release');
        confirming = { id: d.id, action: 'release' };
        return;
      case 'screenshot':
        return screenshot(d);
      case 'mirror':
        return mirror(d);
      case 'stop-mirror':
        return stopMirror(d);
      case 'logs':
        return openLogs(d);
      case 'boot':
        return boot(d);
      case 'shutdown':
        confirming = { id: d.id, action: 'shutdown' };
        return;
    }
  }

  function confirmText(d: DebugDevice, action: 'release' | 'shutdown'): string {
    if (action === 'shutdown') {
      return d.claimed_by
        ? `Stop ${d.title}? ${holderName(d.claimed_by)} holds it and loses the running device.`
        : `Stop ${d.title}? Whatever runs on it closes.`;
    }
    const next = d.queue?.[0];
    return `Release ${d.title}? ${holderName(d.claimed_by ?? '')} loses the claim; ${
      next ? `${holderName(next.holder)}, next in line, gets it` : 'the next in line gets it'
    }.`;
  }

  const enlargedShot = $derived(enlarged !== null ? shots[enlarged] : undefined);
</script>

<section class="board" data-testid="debug-devices-board" aria-label="Device board">
  <p class="intro">
    Every phone, emulator and simulator your hosts see. Claim one while you test by hand, or wait in line
    for one a session holds; the table below edits labels, sharing and installs.
  </p>

  <div class="head">
    <span class="count" data-testid="device-board-count">{devices ? countLine(devices) : ''}</span>
    {#if loadedAt !== null}
      <span class="age" data-testid="device-board-age">updated {timeAgo(loadedAt / 1000, nowMs)}</span>
    {/if}
    <span class="grow"></span>
    <Button size="sm" busy={rescanning} busyLabel="Rescanning…" onclick={rescan} testid="device-rescan"
      ><Icon name="retry" size={12} />Rescan all</Button
    >
  </div>

  <div class="filters" role="toolbar" aria-label="Filter devices">
    <div class="chips" role="group" aria-label="State">
      {#each STATE_FILTERS as f (f.id)}
        <button
          class="chip"
          class:on={stateFilter === f.id}
          aria-pressed={stateFilter === f.id}
          onclick={() => (stateFilter = f.id)}
          data-testid="device-filter-{f.id}">{f.label}</button
        >
      {/each}
    </div>
    <div class="chips" role="group" aria-label="Platform">
      {#each PLATFORM_FILTERS as f (f.id)}
        <button
          class="chip"
          class:on={platform === f.id}
          aria-pressed={platform === f.id}
          onclick={() => (platform = platform === f.id ? 'any' : f.id)}
          data-testid="device-platform-{f.id}">{f.label}</button
        >
      {/each}
    </div>
    <label class="search">
      <Icon name="search" size={14} />
      <input
        type="search"
        aria-label="Search devices"
        placeholder="Name, host or model"
        bind:value={query}
        data-testid="device-search"
      />
    </label>
  </div>

  {#if scanError}
    <p class="err" role="alert" data-testid="device-scan-error">Rescan failed: {scanError}</p>
  {/if}

  {#if loadError}
    <div class="state-box failed" role="alert" data-testid="device-board-error">
      <span>Could not list the devices: {loadError.message}</span>
      <Button size="sm" busy={loading} busyLabel="Retrying…" onclick={load} testid="device-board-retry">Retry</Button>
    </div>
  {/if}

  {#if devices === null}
    {#if !loadError}
      <div class="state-box" data-testid="device-board-loading">
        <Loader name="comet" size={14} />Listing devices…
      </div>
    {/if}
  {:else if devices.length === 0}
    <p class="state-box" data-testid="device-board-empty">{EMPTY}</p>
  {:else if shown.length === 0}
    <p class="state-box" data-testid="device-board-nomatch">No device matches these filters.</p>
  {:else}
    {#each groups as g (g.host)}
      <section class="host" data-testid="device-host-{g.host}" aria-label="Devices on {g.host}">
        <h4 class="host-head">
          <Icon name="hosts" size={14} />{g.host}
          <span class="meta">{g.devices.length} {g.devices.length === 1 ? 'device' : 'devices'} · last seen {timeAgo(g.lastSeen, nowMs)}</span>
        </h4>
        <div class="grid">
          {#each g.devices as d (d.id)}
            {@const acts = cardActions(d, !!mirroring[d.id])}
            {@const hint = d.ready ? null : stateHint(d)}
            {@const waiting = waitingOf(d)}
            {@const cardBusy = busy[d.id] ?? null}
            {@const shot = shots[d.id]}
            <article class="card" class:not-ready={!d.ready} data-testid="device-card-{d.id}" aria-label={d.title}>
              <div class="top">
                <svg
                  class="of-ico glyph"
                  width="22"
                  height="22"
                  viewBox="0 0 16 16"
                  role="img"
                  aria-label={d.platform === 'ios' ? 'iOS' : d.platform === 'android' ? 'Android' : 'Device'}
                  data-platform={d.platform}
                >
                  <rect x="4.5" y="1.5" width="7" height="13" rx="1.5" />
                  {#if d.platform === 'ios'}<path d="M7 12.5h2" />{:else if d.platform === 'android'}<path
                      d="M6.5 3.5h3"
                    /><circle cx="8" cy="12.3" r="0.5" />{/if}
                </svg>
                <div class="names">
                  <b class="title">{d.title}</b>
                  <span class="sub">{deviceSubtitle(d)}</span>
                </div>
                <div class="badges">
                  <StatusChip state={stateTone(d)} label={stateLabel(d)} testid="device-state-{d.id}" />
                  {#if d.shared}<span class="of-chip" data-testid="device-shared-{d.id}">shared</span>{/if}
                </div>
              </div>

              {#if hint}<p class="hint" data-testid="device-hint-{d.id}">{hint}</p>{/if}

              <p class="claim" class:free={!d.claimed_by} data-testid="device-claim-{d.id}">{claimLine(d, nowSecs)}</p>

              {#if waiting > 0 || d.queue_place}
                <div class="queue" data-testid="device-queue-{d.id}">
                  {#if waiting > 0}
                    <button
                      class="link-btn"
                      aria-expanded={!!linesOpen[d.id]}
                      onclick={() => (linesOpen[d.id] = !linesOpen[d.id])}
                      data-testid="device-queue-toggle-{d.id}">{waiting} waiting</button
                    >
                  {/if}
                  {#if d.queue_place}
                    <span class="place" data-testid="device-place-{d.id}">You're {placeName(d.queue_place)}</span>
                  {/if}
                  {#if linesOpen[d.id] && d.queue?.length}
                    <ol class="waiters" data-testid="device-waiters-{d.id}">
                      {#each d.queue as w (w.holder)}
                        <li>{holderName(w.holder)}{#if w.note}<span class="meta">{` · ${w.note}`}</span>{/if}</li>
                      {/each}
                    </ol>
                  {/if}
                </div>
              {/if}

              {#if d.ready}
                <div class="shot" data-testid="device-shot-{d.id}">
                  {#if shot}
                    <button class="thumb" onclick={() => (enlarged = d.id)} aria-label="Enlarge the screenshot of {d.title}">
                      <img src={screenshotSrc(shot)} alt={shot.caption} data-testid="device-shot-img-{d.id}" />
                    </button>
                  {/if}
                  <Button
                    size="sm"
                    variant="quiet"
                    busy={cardBusy === 'screenshot'}
                    busyLabel="Capturing…"
                    disabled={!!cardBusy}
                    onclick={() => screenshot(d)}
                    testid="device-action-{d.id}-screenshot">{shot ? 'Refresh' : 'Screenshot'}</Button
                  >
                </div>
              {/if}

              {#if acts.includes('claim') || acts.includes('queue')}
                <input
                  class="note"
                  aria-label="Note for {d.title} (optional)"
                  placeholder="What for? (optional)"
                  maxlength="200"
                  bind:value={notes[d.id]}
                  data-testid="device-note-{d.id}"
                />
              {/if}

              {#if confirming?.id === d.id}
                <div class="confirm" role="alert" data-testid="device-confirm-{d.id}">
                  <span>{confirmText(d, confirming.action)}</span>
                  <div class="confirm-btns">
                    <Button
                      size="sm"
                      variant="danger"
                      onclick={() => (confirming?.action === 'shutdown' ? shutdown(d) : release(d, 'release'))}
                      testid="device-confirm-{d.id}-yes">{confirming.action === 'shutdown' ? 'Stop' : 'Release'}</Button
                    >
                    <Button size="sm" variant="quiet" onclick={() => (confirming = null)} testid="device-confirm-{d.id}-no"
                      >Cancel</Button
                    >
                  </div>
                </div>
              {/if}

              <div class="actions">
                {#each acts as a (a)}
                  <Button
                    size="sm"
                    variant={a === 'claim' ? 'primary' : 'default'}
                    busy={cardBusy === a}
                    busyLabel={ACTION_LABEL[a][1]}
                    disabled={!!cardBusy}
                    onclick={() => run(d, a)}
                    testid="device-action-{d.id}-{a}">{ACTION_LABEL[a][0]}</Button
                  >
                {/each}
              </div>

              {#if mirrorOut[d.id]}
                <pre class="out" data-testid="device-mirror-out-{d.id}">{mirrorOut[d.id]}</pre>
              {/if}
              {#if errors[d.id]}
                <p class="err" role="alert" data-testid="device-error-{d.id}">{errors[d.id]}</p>
              {/if}
            </article>
          {/each}
        </div>
      </section>
    {/each}
  {/if}
</section>

{#if enlarged !== null && enlargedShot}
  <Modal
    title={`Screenshot · ${byId(enlarged)?.title ?? ''}`}
    onclose={() => (enlarged = null)}
    maxWidth="min(90vw, 900px)"
    testid="device-shot-overlay"
  >
    <img class="big" src={screenshotSrc(enlargedShot)} alt={enlargedShot.caption} />
    <p class="meta">{enlargedShot.caption}</p>
  </Modal>
{/if}

{#if logs}
  <Modal title={`Logs · ${logs.title}`} onclose={() => (logs = null)} width="min(90vw, 860px)" testid="device-logs">
    {#if logs.out.exit_code !== 0}
      <p class="err" data-testid="device-logs-exit">The log command ended with exit code {logs.out.exit_code}.</p>
    {/if}
    {#if logs.out.output}
      <pre class="log" data-testid="device-logs-output">{logs.out.output}</pre>
    {:else}
      <p class="meta" data-testid="device-logs-empty">No log lines.</p>
    {/if}
    {#if logs.out.truncated}
      <p class="meta" data-testid="device-logs-truncated">Cut: only the last lines are shown.</p>
    {/if}
  </Modal>
{/if}

<style>
  .board {
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    margin-bottom: var(--space-4);
  }
  .intro,
  .meta,
  .age,
  .sub,
  .hint {
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .intro {
    margin: 0;
  }
  .head {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    flex-wrap: wrap;
  }
  .count {
    font-size: var(--text-sm);
    font-weight: 500;
  }
  .grow {
    flex: 1 1 auto;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: center;
  }
  .chips {
    display: inline-flex;
    gap: var(--space-1);
  }
  .chip {
    height: 22px;
    padding: 0 8px;
    border-radius: var(--radius-pill);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-2);
    font: inherit;
    font-size: var(--text-2xs);
    cursor: pointer;
  }
  .chip:hover {
    background: var(--bg-hover);
  }
  .chip.on {
    background: var(--accent-soft);
    border-color: var(--accent);
    color: var(--fg);
  }
  .search {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    flex: 1 1 180px;
    max-width: 280px;
    padding: 0 var(--space-2);
    height: var(--control-h);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    color: var(--fg-muted);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: transparent;
    color: var(--fg);
    font: inherit;
    font-size: var(--text-xs);
    outline: none;
  }
  .state-box {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-3);
    border: 1px dashed var(--border);
    border-radius: var(--radius-md);
    font-size: var(--text-xs);
    color: var(--fg-muted);
    margin: 0;
  }
  .state-box.failed {
    border-style: solid;
    border-color: var(--status-failed);
    color: var(--fg);
    justify-content: space-between;
  }
  .host-head {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    margin: 0 0 var(--space-2);
    font-size: var(--text-sm);
  }
  .host-head .meta {
    font-weight: 400;
    margin-left: var(--space-1);
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: var(--space-2);
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
    min-width: 0;
  }
  .card.not-ready {
    background: var(--bg-sunk, var(--bg-pane));
  }
  .top {
    display: flex;
    gap: var(--space-2);
    align-items: flex-start;
  }
  .glyph {
    color: var(--fg-2);
    margin-top: 1px;
  }
  .names {
    display: flex;
    flex-direction: column;
    min-width: 0;
    flex: 1;
  }
  .title {
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }
  .badges {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 4px;
  }
  .hint,
  .claim {
    margin: 0;
  }
  .claim {
    font-size: var(--text-xs);
  }
  .claim.free {
    color: var(--status-done);
  }
  .queue {
    font-size: var(--text-xs);
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: baseline;
  }
  .place {
    color: var(--status-waiting);
  }
  .link-btn {
    border: 0;
    background: none;
    padding: 0;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
    text-decoration: underline dotted;
  }
  .waiters {
    flex-basis: 100%;
    margin: 0;
    padding-left: 1.4em;
  }
  .shot {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-height: 28px;
  }
  .thumb {
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    cursor: zoom-in;
    line-height: 0;
  }
  .thumb img {
    max-height: 120px;
    max-width: 120px;
    object-fit: contain;
    border-radius: var(--radius-sm);
  }
  .note {
    font: inherit;
    font-size: var(--text-xs);
    padding: 2px 6px;
    height: 24px;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-sm);
    background: var(--bg);
    color: var(--fg);
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }
  .confirm {
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-2);
    border-radius: var(--radius-sm);
    background: var(--waiting-soft);
    font-size: var(--text-xs);
  }
  .confirm-btns {
    display: flex;
    gap: var(--space-1);
  }
  .err {
    margin: 0;
    font-size: var(--text-2xs);
    color: var(--status-failed, var(--danger));
    overflow-wrap: anywhere;
  }
  .out,
  .log {
    margin: 0;
    font-family: var(--font-mono, monospace);
    font-size: var(--text-2xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .out {
    max-height: 6em;
    overflow: auto;
    color: var(--fg-muted);
  }
  .log {
    max-height: 60vh;
    overflow: auto;
    padding: var(--space-2);
    background: var(--bg-sunk, var(--bg));
    border-radius: var(--radius-sm);
  }
  .big {
    display: block;
    max-width: 100%;
    max-height: 70vh;
    margin: 0 auto;
    object-fit: contain;
  }
</style>
