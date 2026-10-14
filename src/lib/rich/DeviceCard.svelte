<script lang="ts">
  // A `device` block (docs/chat-blocks.md, docs/debug-devices.md): a debug
  // device the agent points at, drawn live from the person's own device
  // list. The block names the device only; its state, holder and line are
  // the app's, read once on mount and again on Refresh. Screenshot is a
  // read and works in any conversation; Mirror opens a window on this
  // machine and is offered only in the live one.
  import { onMount } from 'svelte';
  import { errorText } from '../error_copy';
  import type { IpcError } from '../result';
  import Button from '../kit/Button.svelte';
  import Markdown from '../MarkdownView.svelte';
  import DeviceSummary from '../DeviceSummary.svelte';
  import ImageShot from '../ImageShot.svelte';
  import {
    canMirror,
    debugDeviceScreenshot,
    listDebugDevices,
    resolveDevice,
    startDeviceMirror,
    type DebugDevice,
    type Screenshot,
  } from '../debug_devices';
  import type { UiBlock } from '../rich_blocks';

  let { block, live = true }: { block: Extract<UiBlock, { kind: 'device' }>; live?: boolean } = $props();

  /** null until the first list answers. */
  let device = $state<DebugDevice | null | undefined>(undefined);
  let loading = $state(false);
  let failure = $state<string | null>(null);
  let shot = $state<Screenshot | null>(null);
  let shooting = $state(false);
  let mirroring = $state(false);
  let mirrorNote = $state<string | null>(null);
  let nowSecs = $state(Date.now() / 1000);

  // The backend's own words: they name the holder, the missing tool, the
  // adb versions; the generic sentence only stands in for an empty one.
  const ipcMessage = (e: IpcError) => e.message?.trim() || errorText(e);

  async function load() {
    loading = true;
    failure = null;
    const r = await listDebugDevices();
    loading = false;
    nowSecs = Date.now() / 1000;
    if (r.ok) device = resolveDevice(r.value, block.device);
    else failure = ipcMessage(r.error);
  }

  async function screenshot() {
    if (!device) return;
    shooting = true;
    failure = null;
    const r = await debugDeviceScreenshot(device.id);
    shooting = false;
    if (r.ok) shot = r.value;
    else failure = ipcMessage(r.error);
  }

  async function mirror() {
    if (!device) return;
    mirroring = true;
    failure = null;
    mirrorNote = null;
    const r = await startDeviceMirror(device.id);
    mirroring = false;
    if (r.ok) mirrorNote = `Mirroring ${r.value.title} in a scrcpy window.`;
    else failure = ipcMessage(r.error);
  }

  onMount(() => void load());
</script>

<section class="card" data-testid="rich-device" aria-label="Debug device {block.title ?? block.device}">
  <header>
    <span class="tag">Debug device</span>
    <button type="button" class="link" disabled={loading} data-testid="rich-device-refresh" onclick={() => void load()}>Refresh</button>
  </header>
  {#if device}
    <DeviceSummary {device} {nowSecs} heading={block.title} />
  {:else if device === null}
    {#if block.title}<strong class="title">{block.title}</strong>{/if}
    <p class="none" data-testid="rich-device-none">No debug device <code>{block.device}</code> on this fleet.</p>
  {:else if !failure}
    <p class="none">Looking for <code>{block.device}</code>…</p>
  {/if}
  {#if block.note}<div class="note"><Markdown source={block.note} /></div>{/if}
  {#if device}
    <div class="actions">
      <Button size="sm" onclick={() => void screenshot()} busy={shooting} busyLabel="Taking…" testid="rich-device-screenshot"
        >{shot ? 'Refresh screenshot' : 'Screenshot'}</Button
      >
      {#if canMirror(device)}
        <Button
          size="sm"
          onclick={() => void mirror()}
          busy={mirroring}
          busyLabel="Starting…"
          disabled={!live}
          title={live ? 'Open a live scrcpy window of it on this machine' : 'Mirror from the live conversation or the Debug devices page'}
          testid="rich-device-mirror">Mirror</Button
        >
      {/if}
    </div>
  {/if}
  {#if mirrorNote}<p class="ok" role="status">{mirrorNote}</p>{/if}
  {#if failure}<p class="err" role="alert" data-testid="rich-device-error">{failure}</p>{/if}
  {#if shot}
    <ImageShot mime={shot.mime} data={shot.data} alt={shot.caption} testid="rich-device-shot" />
  {/if}
</section>

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    margin: 0.4em 0 0.7em;
    padding: 0.65rem 0.8rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background: var(--bg-pane);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 0.5rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  .tag {
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .link {
    font: inherit;
    color: var(--fg-muted);
    background: none;
    border: none;
    padding: 0;
    cursor: pointer;
  }
  .link:hover:not(:disabled) {
    color: var(--fg);
    text-decoration: underline;
  }
  .link:disabled {
    cursor: default;
    opacity: 0.6;
  }
  .title {
    font-size: var(--text-sm);
  }
  .none,
  .ok,
  .err {
    margin: 0;
    font-size: var(--text-xs);
  }
  .none {
    color: var(--fg-muted);
  }
  .note {
    font-size: var(--text-xs);
    color: var(--fg-2);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .ok {
    color: var(--fg-2);
  }
  .err {
    color: var(--status-failed);
  }
</style>
