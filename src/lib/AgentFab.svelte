<script lang="ts">
  // The one visible way in, from every view. Mounted once in App.svelte
  // beside HintLayer and McpConfirmDialog — that is what makes it present
  // over the terminal, Hosts and Files without any of them knowing.
  //
  // The button itself never disables: for `no_mcp` / `token_revoked` the
  // title carries the explanation, but opening the panel is how the person
  // reads the full story (blockedCopy's title text, and for those two
  // states no action button under it). It does not disable while waking
  // either — a press then is a press to CLOSE, and disabling the only way
  // out of a sheet that is busy being born is the wound this button already
  // had once.
  import {
    toggleAgent,
    agentPanelOpen,
    operatorState,
    blockedCopy,
    type OperatorBlocked,
  } from './operator';
  import { agentChordLabel } from './app_views';
  import { detectMac } from './terminal_keys';
  import { hintAnchor } from './hints';

  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const blocked = $derived(
    $operatorState === 'no_mcp' || $operatorState === 'token_revoked'
      ? blockedCopy($operatorState as OperatorBlocked)
      : null,
  );
  const title = $derived(
    blocked
      ? blocked.title
      : `${$agentPanelOpen ? 'Close the agent' : 'Ask the agent'} (${agentChordLabel(isMac)})`,
  );
</script>

<button
  class="agent-fab"
  {title}
  aria-label="Agent"
  aria-expanded={$agentPanelOpen}
  onclick={() => void toggleAgent()}
  use:hintAnchor={{ id: 'agent-fab', when: !blocked }}
>
  <span aria-hidden="true">✦</span>
</button>

<style>
  .agent-fab {
    position: fixed;
    right: 20px;
    /* Clear of the status footer — it used to cover the usage button. */
    bottom: calc(var(--status-h) + var(--layer-gap));
    width: var(--fab-size);
    height: var(--fab-size);
    border-radius: 50%;
    border: 1px solid var(--border);
    background: var(--bg-pane);
    color: var(--fg);
    font-size: 20px;
    cursor: pointer;
    z-index: 40;
    box-shadow: 0 2px 10px rgb(0 0 0 / 30%);
  }
  .agent-fab:hover {
    border-color: var(--accent);
  }
</style>
