<!-- A key hint beside the action it triggers (manual: Kbd). Takes the Mac
     chord; off the Mac it shows the platform chord and puts the Mac one in
     the title. -->
<script lang="ts">
  import { detectMac } from '../terminal_keys';
  import { platformChord } from './kbd';

  let {
    chord,
    mac = detectMac(typeof navigator === 'undefined' ? undefined : navigator),
  }: { chord: string; mac?: boolean } = $props();

  const shown = $derived(platformChord(chord, mac));
</script>

<span class="of-kbd" title={shown === chord ? undefined : chord}>{shown}</span>
