<script lang="ts">
  // Add host on a desktop paired with a hub: the Add host form (the same
  // fleet.form/1 spec the chat opens) over the HUB's ~/.ssh/config, run by
  // the hub's `add_host`, which probes the alias over its own SSH and keeps
  // the host only if it answers. The guided wizard (AddHostWizard) checks
  // and drafts on this machine, which would test the wrong route; the phone
  // adds hosts this way too. The hub asks for its owner's trusted full
  // device and says so (`fleet-hub client trust <name>`) to any other.
  import { onMount } from 'svelte';
  import WizardDialog from './forms/WizardDialog.svelte';
  import { addHostWizard, runAddHost } from './forms/add_host_wizard';
  import { discoverHosts } from './hosts';
  import type { FieldProblem, Values } from './forms/forms';
  import type { Wizard } from './forms/wizards';

  let { onClose }: { onClose: () => void } = $props();

  let wizard = $state<Wizard>(addHostWizard([]));
  let busy = $state(false);
  let error = $state<string | null>(null);
  let problems = $state<FieldProblem[]>([]);

  onMount(async () => {
    const r = await discoverHosts();
    // No list is not a dead end: "Another alias" still takes a typed one.
    if (r.ok) wizard = addHostWizard(r.value ?? []);
  });

  async function run(v: Values) {
    busy = true;
    error = null;
    problems = [];
    const out = await runAddHost(v);
    busy = false;
    if (out.ok) return onClose();
    error = out.error ?? null;
    problems = out.problems ?? [];
  }
</script>

<WizardDialog {wizard} {busy} {error} {problems} errorTestid="hub-add-host-error" run={(v) => void run(v)} onclose={onClose} />
