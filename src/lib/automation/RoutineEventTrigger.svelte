<script lang="ts">
  // The routine editor's "On an event" fields (Orbit Fleet M15 step G2.4,
  // the FormsAutomation board's "Starts on an event"): the event, session
  // ones and the pull request ones (review, checks, merge), then for a pull
  // request "Only when repo / author", and for any event "Not more often
  // than once per PR (session) per …". The hub checks every field again
  // (`service::routines::event_filters`); this only offers what it accepts.
  import { EVENT_CHOICES, RATE_CHOICES, eventLabel, isPrEvent, type EventFilter } from '../routines';

  interface Props {
    event: string;
    filter: EventFilter;
    /** `owner/repo` of the fleet's projects, offered for the repo filter. */
    repos?: readonly string[];
  }
  let { event = $bindable(), filter = $bindable(), repos = [] }: Props = $props();

  const pr = $derived(isPrEvent(event));
</script>

<label class="field"
  >Event
  <select data-testid="routine-event" bind:value={event}>
    {#each EVENT_CHOICES as e (e)}<option value={e}>{eventLabel(e)}</option>{/each}
  </select>
</label>
{#if pr}
  <div class="pair">
    <label class="field"
      >Only when repo
      <input
        data-testid="routine-event-repo"
        list="routine-event-repos"
        bind:value={filter.repo}
        placeholder="any repo"
        spellcheck="false"
        maxlength="200"
      />
      <datalist id="routine-event-repos">
        {#each repos as r (r)}<option value={r}></option>{/each}
      </datalist>
    </label>
    <label class="field"
      >Author
      <select data-testid="routine-event-author" bind:value={filter.author}>
        <option value="me">me</option>
        <option value="anyone">anyone in its organisation</option>
      </select>
    </label>
  </div>
{/if}
<label class="field"
  >Not more often than
  <select data-testid="routine-event-rate" bind:value={filter.rate}>
    {#each RATE_CHOICES as c (c.label)}
      <option value={c.secs ? String(c.secs) : ''}
        >{c.secs ? c.label.replace('once per', `once per ${pr ? 'PR' : 'session'} per`) : c.label}</option
      >
    {/each}
  </select>
  {#if pr}<span class="hint">The run's prompt names the pull request that started it.</span>{/if}
</label>

<style>
  .field { display: flex; flex-direction: column; gap: 4px; font-size: var(--text-sm); }
  .pair { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-2); }
  .hint { color: var(--fg-muted); font-size: var(--text-xs); }
</style>
