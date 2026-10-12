<script lang="ts">
  // Work graph M7.3: "Show what auto-tidy would do" — the current tidy
  // candidates that auto-tidy would act on with the reasons ticked in
  // `work.auto_tidy_reasons`. A registered custom item on the generated
  // Work graph page (declarative pages P3) until a data source can say it.
  import { fleetSettings, parseAutoTidyReasons, settingBool, SETTING_KEYS } from './fleet_settings';
  import { autoTidyPreview, formatIdle, refreshTidy, tidyReasonLabel, tidyReport, type TidyCandidate } from './tidy';

  let dryRun = $state<TidyCandidate[] | null>(null);
  let busy = $state(false);

  async function showDryRun() {
    busy = true;
    await refreshTidy();
    busy = false;
    dryRun = autoTidyPreview(
      $tidyReport.candidates,
      parseAutoTidyReasons($fleetSettings[SETTING_KEYS.workAutoTidyReasons]),
    );
  }
</script>

<div class="auto-tidy-preview">
  <button class="btn" type="button" data-testid="work-auto-tidy-dry-run" disabled={busy} onclick={() => void showDryRun()}
    >Show what auto-tidy would do</button
  >
  {#if dryRun !== null}
    <div class="preview" data-testid="work-auto-tidy-preview">
      {#if dryRun.length === 0}
        Nothing right now.
      {:else}
        Auto-tidy would {settingBool($fleetSettings, SETTING_KEYS.workAutoTidy) ? '' : '(once turned on) '}safe-kill or archive:
        <ul>
          {#each dryRun as c (c.session_id)}
            <li data-testid="work-auto-tidy-preview-row">
              {c.label || c.tmux_name} on {c.host_alias}{c.key ? ` · ${c.key}` : ''} — {tidyReasonLabel(c.reason)}, idle {formatIdle(c.idle_secs)}
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</div>

<style>
  .auto-tidy-preview {
    padding: 0.35rem 0;
  }
  .preview {
    margin-top: 0.4rem;
    font-size: var(--text-2xs);
    color: var(--fg-muted);
  }
  ul {
    margin: 0.25rem 0 0;
    padding-left: 1.1rem;
  }
</style>
