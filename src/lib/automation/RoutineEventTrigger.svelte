<script lang="ts">
  // The routine editor's "On an event" fields (Orbit Fleet M15 step G2.4,
  // the FormsAutomation board's "Starts on an event"): the event, session
  // ones and the pull request ones (review, checks, merge), then for a pull
  // request "Only when repo / author", for an event about a host "Only on
  // host", and for any event "Not more often than once per PR (session,
  // host, …) per …". The events come in three groups: sessions, pull
  // requests, and the fleet's hosts, accounts and missions (migration 201).
  // The hub checks every field again (`service::routines::event_filters`);
  // this only offers what it accepts.
  import { EVENT_GROUPS, RATE_CHOICES, eventHostChoices, eventLabel, isFleetEvent, isPrEvent, rateSubject, takesHostFilter, type EventFilter } from '../routines';
  import { hosts } from '../hosts';

  interface Props {
    event: string;
    filter: EventFilter;
    /** `owner/repo` of the fleet's projects, offered for the repo filter. */
    repos?: readonly string[];
    /** The routine's own host: a fleet event's "Only on host" offers only
     *  the hosts of its org, as the hub refuses the rest. */
    routineHost?: string;
  }
  let { event = $bindable(), filter = $bindable(), repos = [], routineHost }: Props = $props();

  const pr = $derived(isPrEvent(event));
  const per = $derived(rateSubject(event));
  const hostAliases = $derived(eventHostChoices($hosts, event, routineHost));
</script>

<label class="field"
  >Event
  <select data-testid="routine-event" bind:value={event}>
    {#each EVENT_GROUPS as g (g.label)}
      <optgroup label={g.label}>
        {#each g.events as e (e)}<option value={e}>{eventLabel(e)}</option>{/each}
      </optgroup>
    {/each}
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
{#if takesHostFilter(event)}
  <label class="field"
    >Only on host
    <select data-testid="routine-event-host" bind:value={filter.host}>
      <option value="">any host</option>
      {#each hostAliases as h (h)}<option value={h}>{h}</option>{/each}
      {#if filter.host && !hostAliases.includes(filter.host)}<option value={filter.host}>{filter.host}</option>{/if}
    </select>
    {#if event === 'session_moved'}<span class="hint">A move matches the host the session moved to.</span>{/if}
  </label>
{/if}
<label class="field"
  >Not more often than
  <select data-testid="routine-event-rate" bind:value={filter.rate}>
    {#each RATE_CHOICES as c (c.label)}
      <option value={c.secs ? String(c.secs) : ''}
        >{c.secs ? c.label.replace('once per', `once per ${per} per`) : c.label}</option
      >
    {/each}
  </select>
  {#if pr}<span class="hint">The run's prompt names the pull request that started it.</span>{/if}
  {#if !pr && !isFleetEvent(event)}<span class="hint">The run's prompt names the session that started it and what happened to it.</span>{/if}
  {#if isFleetEvent(event)}<span class="hint">Fires once when it happens, not again while it lasts. The run's prompt says what happened.</span>{/if}
</label>

<style>
  .field { display: flex; flex-direction: column; gap: 4px; font-size: var(--text-sm); }
  .pair { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-2); }
  .hint { color: var(--fg-muted); font-size: var(--text-xs); }
</style>
