<!-- A key hint beside the action it triggers (manual: Kbd). Takes the Mac
     chord; off the Mac it shows the platform chord and puts the Mac one in
     the title. A `shortcut` (a SHORTCUTS id) takes the platform's binding
     from the registry instead, for chords that are not ⌘→Ctrl off the Mac. -->
<script lang="ts">
  import { detectMac } from '../terminal_keys';
  import { platformChord } from './kbd';
  import { shortcutLabel } from '../shortcuts';

  let {
    chord,
    shortcut,
    mac = detectMac(typeof navigator === 'undefined' ? undefined : navigator),
  }: { chord: string; shortcut?: string; mac?: boolean } = $props();

  const shown = $derived(shortcut ? shortcutLabel(shortcut, mac) : platformChord(chord, mac));
</script>

<span class="of-kbd" title={shown === chord ? undefined : chord}>{shown}</span>
