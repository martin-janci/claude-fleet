<script lang="ts">
  // Host picker chips with NewSessionDialog's rule: every visible host, and
  // an unreachable one disabled unless it is `local`. `lockedReason` lets the
  // owner disable more (with the reason as the chip's title).
  import { hosts } from './hosts';

  let {
    active,
    labelId,
    disabled = false,
    lockedReason = () => null,
    onpick,
  }: {
    active: string;
    /** DOM id for the "Host" caption that names the group. */
    labelId: string;
    /** Disable every chip (e.g. while a request is in flight). */
    disabled?: boolean;
    lockedReason?: (alias: string) => string | null;
    onpick: (alias: string) => void;
  } = $props();
</script>

<span class="label" id={labelId}>Host</span>
<div class="host-row" role="group" aria-labelledby={labelId}>
  {#each $hosts.filter((h) => !h.hidden) as h (h.alias)}
    {@const locked = lockedReason(h.alias)}
    <button
      type="button"
      class="host-pick"
      class:active={active === h.alias}
      aria-pressed={active === h.alias}
      disabled={disabled || (!h.reachable && h.alias !== 'local') || locked !== null}
      title={locked ?? undefined}
      onclick={() => onpick(h.alias)}
    >{h.alias}</button>
  {/each}
</div>

<style>
  .label { font-size: 0.7rem; color: var(--fg-muted); text-transform: uppercase; }
  .host-row {
    display: flex;
    gap: 0.3rem;
    flex-wrap: wrap;
    max-height: 5.2rem;
    overflow-y: auto;
  }
  .host-pick {
    font-size: 0.75rem;
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--fg-muted);
    border-radius: 999px;
    cursor: pointer;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .host-pick.active { color: var(--fg); border-color: var(--accent); }
  .host-pick:disabled { opacity: 0.4; cursor: not-allowed; }
</style>
