<script lang="ts">
  // The one visible way in, from every view. Mounted once in App.svelte
  // beside HintLayer and McpConfirmDialog — that is what makes it present
  // over the terminal, Hosts and Files without any of them knowing.
  //
  // The button itself never disables: for `no_mcp` / `token_revoked` the
  // title carries the explanation, but opening the panel is how the person
  // reads the full story (blockedCopy's title text, and for those two
  // states no action button under it).
  import { openAgent, operatorState, blockedCopy, type OperatorBlocked } from './operator';
  import { agentChordLabel } from './app_views';
  import { detectMac } from './terminal_keys';
  import { hintAnchor } from './hints';

  const isMac = detectMac(typeof navigator === 'undefined' ? undefined : navigator);
  const blocked = $derived(
    $operatorState === 'no_mcp' || $operatorState === 'token_revoked'
      ? blockedCopy($operatorState as OperatorBlocked)
      : null,
  );
  const title = $derived(blocked ? blocked.title : `Ask the agent (${agentChordLabel(isMac)})`);
</script>

<button
  class="agent-fab"
  {title}
  aria-label="Agent"
  onclick={() => void openAgent()}
  use:hintAnchor={{ id: 'agent-fab', when: !blocked }}
>
  <span aria-hidden="true">✦</span>
</button>

<style>
  .agent-fab {
    position: fixed;
    right: 20px;
    bottom: 20px;
    width: 48px;
    height: 48px;
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
