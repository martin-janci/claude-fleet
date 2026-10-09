<!--
  The New layout's first run (Orbit Fleet redesign step 10.5): the tour,
  which starts once by itself after the welcome and the startup loads, and
  the Get started checklist in the corner until it is hidden. App mounts it
  under the New layout only; Classic keeps the sidebar's onboarding card.
-->
<script lang="ts">
  import Tour from './Tour.svelte';
  import GetStarted from './GetStarted.svelte';
  import { onboardingDismissed, onboardingWelcomed } from './onboarding';
  import { startupFacts } from './startup';
  import { startTour, tourSeen, tourStep } from './tour';

  let { mac = false }: { mac?: boolean } = $props();

  $effect(() => {
    if ($startupFacts.done && $onboardingWelcomed && !$tourSeen && $tourStep === null) startTour();
  });
</script>

<Tour {mac} />
{#if !$onboardingDismissed && $tourStep === null}
  <GetStarted />
{/if}
