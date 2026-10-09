<!--
  The rail (Orbit Fleet redesign step 3.2), drawn by the kit's
  Rail (design manual: Rail). The order and which items show live in
  `rail.ts`; this file names the current one, the titles with their
  shortcuts, and the Inbox's Needs you count. The shortcuts themselves stay
  where they were (`shortcuts.ts`).
-->
<script lang="ts">
  import { destination } from './destination';
  import { sidebarView } from './work_view';
  import { settingsOpen } from './app_views';
  import { currentRailItem, visibleRailItems, type RailId, type RailItem } from './rail';
  import { shortcutLabel } from './shortcuts';
  import { inboxCount } from './inbox';
  import Rail from './kit/Rail.svelte';
  import { hintAnchor } from './hints';

  interface Props {
    isMac: boolean;
    onselect: (id: RailId) => void;
  }
  let { isMac, onselect }: Props = $props();

  const items = visibleRailItems();

  const current = $derived($settingsOpen ? 'settings' : currentRailItem($destination, $sidebarView));

  const TITLE_NAME: Partial<Record<RailId, string>> = { accounts: 'Accounts & hosts' };

  function title(item: RailItem): string {
    const name = TITLE_NAME[item.id] ?? item.label;
    return item.shortcut ? `${name}  ${shortcutLabel(item.shortcut, isMac)}` : name;
  }

  // The rail ids are the manual's icon names (kit/icons.ts).
  const entries = $derived(
    items.map((item) => ({
      id: item.id,
      label: item.label,
      icon: item.id,
      title: title(item),
      badge: item.id === 'inbox' && $inboxCount > 0 ? $inboxCount : undefined,
      bottom: item.id === 'settings',
    })),
  );

  // The agent hint anchors on Control, where the floating button used to be
  // (13.1). The kit's Rail draws the link, so the anchor is found in it.
  function controlHint(host: HTMLElement) {
    const el = host.querySelector<HTMLElement>('[data-rail="control"]');
    return el ? hintAnchor(el, { id: 'agent-fab' }) : undefined;
  }
</script>

<div class="rail-host" use:controlHint>
  <Rail items={entries} current={current ?? undefined} onselect={(id) => onselect(id as RailId)} testid="rail" />
</div>

<style>
  /* Layout only: the rail fills its grid row and keeps Settings at the bottom. */
  .rail-host {
    display: flex;
    height: 100%;
    min-width: 0;
    overflow: hidden;
  }
  .rail-host > :global(.of-rail) {
    box-sizing: border-box;
    height: 100%;
    overflow: hidden;
  }
</style>
