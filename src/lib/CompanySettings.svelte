<script lang="ts">
  // Settings › Company (Organisations, Devices, People, Groups) on any fleet
  // this window shows. Each page is a declarative resource page; this wrapper
  // decides, per fleet, what this device may change there
  // (`company_access.ts`: the hub's `org_admin` rule — a fleet admin's
  // unbound device or an org admin's, and trusted `full` to change anything)
  // and, with more than one fleet, which fleet's company it shows. An extra
  // fleet's list and actions are routed by `fleet` (multi-hub design D6).
  import PageView from './pages/PageView.svelte';
  import FleetPicker from './FleetPicker.svelte';
  import type { Descriptor, Page, PageAction, SourceSpec } from './pages/pages';
  import type { ResourceType } from './pages/resources';
  import { fleetPlace, fleets, PRIMARY } from './fleets';
  import { hubStatus } from './hub';
  import {
    NO_COMPANY,
    OWN_COMPANY,
    companyReadonlyReason,
    companyRecordWritable,
    companyResourceFor,
    loadCompanyAccess,
    type CompanyAccess,
  } from './company_access';

  let {
    page,
    pages,
    descs,
    values,
    sources,
    resources,
    actions,
    section = null,
    ownsFleet,
    fleet = $bindable(PRIMARY),
    onnavigate,
  }: {
    page: Page;
    pages: Page[];
    descs: Map<string, Descriptor>;
    values: Record<string, string>;
    sources: SourceSpec[];
    resources: ResourceType[];
    actions: PageAction[];
    section?: string | null;
    /** This app owns the primary fleet (standalone). */
    ownsFleet: boolean;
    /** The fleet whose company is shown: the primary, or an extra's slot.
     *  Bindable, so moving between the Company pages keeps the pick. */
    fleet?: string;
    onnavigate: (pageId: string) => void;
  } = $props();

  let access = $state<CompanyAccess>(NO_COMPANY);
  /** The fleet `access` was read for. Derived against `fleet`, so a switch
   *  hides the page in the same render rather than once more after it. */
  let accessFor = $state<string | null>(null);
  const accessLoaded = $derived(accessFor === fleet);
  /** Bumped by Try again after an access read that did not get through. */
  let retry = $state(0);

  const place = $derived(fleetPlace(fleet, $fleets, $hubStatus));
  const own = $derived(fleet === PRIMARY && ownsFleet);

  $effect(() => {
    const f = fleet;
    void retry;
    if (own) {
      access = OWN_COMPANY;
      accessFor = f;
      return;
    }
    accessFor = null;
    access = NO_COMPANY;
    let stale = false;
    void loadCompanyAccess(f).then((a) => {
      if (stale) return;
      access = a;
      accessFor = f;
    });
    return () => {
      stale = true;
    };
  });

  const resourceId = $derived(page.resource ?? '');
  const shownResources = $derived(resources.map((r) => (r.id === page.resource ? companyResourceFor(r, access) : r)));
  const readonly = $derived(!access.write || !access.admin);
  const reason = $derived(accessLoaded && !own ? companyReadonlyReason(access, place.where, place.clientName) : null);
  const routedFleet = $derived(fleet === PRIMARY ? null : fleet);
</script>

<div class="company-settings" data-testid="company-settings">
  <FleetPicker bind:value={fleet} testid="company-fleet" />

  <!-- Mounted once the fleet's access is known: the page reads its list and
       option sources on mount, and readonly / reason / canChange flow in as
       props, so mounting before the access read would read everything twice. -->
  {#if accessLoaded}
    <PageView
      {page}
      {pages}
      {descs}
      {values}
      {sources}
      resources={shownResources}
      {actions}
      {readonly}
      {reason}
      remote={!own}
      {section}
      fleet={routedFleet}
      canChange={(r) => companyRecordWritable(access, resourceId, r)}
      {onnavigate} />
  {/if}

  {#if accessLoaded && access.error}
    <button type="button" class="btn btn--quiet retry" data-testid="company-access-retry" onclick={() => (retry += 1)}>Try again</button>
  {/if}
</div>

<style>
  .company-settings {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .retry {
    align-self: flex-start;
  }
</style>
