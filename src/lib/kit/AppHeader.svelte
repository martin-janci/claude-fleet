<!-- The 44 px header (manual: AppHeader): the Orbit mark and name, the ⌘K
     command field, then the consumer's account and automation buttons. The
     logo does not animate here. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import Kbd from './Kbd.svelte';
  import OrbitMark from './OrbitMark.svelte';

  let {
    name = 'Orbit Fleet',
    oncommand,
    commandPlaceholder = 'Search or run a command',
    mac,
    children,
    testid,
  }: {
    name?: string;
    /** Opens the quick switcher (⌘K and ⌘P open the same one). */
    oncommand?: () => void;
    commandPlaceholder?: string;
    mac?: boolean;
    /** Accounts (quiet buttons with health dot and quota), a separator, automation. */
    children?: Snippet;
    testid?: string;
  } = $props();
</script>

<header class="of of-header" data-testid={testid}>
  <span class="brand"><OrbitMark size={22} label={null} /><span>{name}</span></span>
  <button class="of-btn command" onclick={oncommand}><span>{commandPlaceholder}</span><Kbd chord="⌘K" {mac} /></button>
  <span class="sp"></span>
  {@render children?.()}
</header>

<style>
  .command {
    width: 260px;
    justify-content: space-between;
    color: var(--fg-muted);
  }
</style>
