// Work graph M12.4: trackers in fleet health, on the desktop.
//
// `fleet_health` (`health_check` here; the hub's `fleet_health` when paired)
// carries a `trackers` roll-up read from the sync's in-memory metrics and the
// store — never a live call. This module turns it into:
//
// - the footer's one-line summary ("trackers: 1 failing · 3 undecided > 7 d");
// - decision D22's Attention items: ONE per failing tracker, deduplicated by
//   tracker id, "Reconnect Jira (acme)", linking to Settings → Work — or,
//   when the roll-up's `reason` says the sync is skipping items it cannot
//   store (work graph M13.1, D25), "Sync skipping items — Jira (acme)": not
//   a credential problem, so never "Reconnect".
//
// Pure helpers plus one store; `TrackerAttention.svelte` polls and renders.
import { setContextRedPct } from './attention';
import { decideHealth } from './decide_health';
import { writable } from 'svelte/store';
import { healthCheck } from './ipc';

export type TrackerHealthLevel = 'ok' | 'degraded' | 'failing';

/** `service::health::TRACKER_REASON_*`. */
export type TrackerHealthReason = 'credential' | 'sync_failed' | 'items_skipped';

/** One tracker's row (`service::health::TrackerHealth`). Null-stripped on the
 *  hub's wire, so every optional field may be absent. */
export interface TrackerHealth {
  tracker_id: number;
  provider?: string;
  name?: string;
  org_id?: number | null;
  org_name?: string | null;
  health?: TrackerHealthLevel | string;
  state?: string;
  consecutive_failures?: number;
  /** Why it is not ok, as data (M13.1): 'credential' | 'sync_failed' |
   *  'items_skipped'; absent from an older hub, and '' while ok. The
   *  Attention wording is picked from this, never from `last_error`. */
  reason?: TrackerHealthReason | string;
  /** Items the last pass skipped. */
  items_failed?: number;
  /** Passes in a row that skipped items. */
  consecutive_partial?: number;
  last_error?: string | null;
  last_success_at?: number | null;
  last_pass_at?: number | null;
  /** Writes fleet gave up on (M13.4e: a PR remote link); absent from an
   *  older hub. Never changes `health`. */
  write_failures?: number;
}

/** `Health.trackers` (`service::health::TrackersHealth`); absent from an
 *  older hub. */
export interface TrackersHealth {
  trackers?: TrackerHealth[];
  failing?: number;
  degraded?: number;
  detection_backlog?: number;
  detection_backlog_days?: number;
}

/** The latest roll-up the desktop has read (null: none yet). */
export const trackersHealth = writable<TrackersHealth | null>(null);

/** Read `health_check` again and publish its roll-up. A failed read keeps the
 *  last one: the footer already reports a health failure, and an Attention
 *  item that blinks out on a hub hiccup would be worse than a stale one. */
export async function refreshTrackersHealth(): Promise<void> {
  const r = await healthCheck();
  if (r.ok) {
    trackersHealth.set(r.value.trackers ?? null);
    decideHealth.set(r.value.decide ?? null);
    setContextRedPct(r.value.context_red_pct);
  }
}

const PROVIDER_SHORT: Record<string, string> = {
  jira: 'Jira',
  jira_dc: 'Jira',
  github: 'GitHub',
  asana: 'Asana',
  linear: 'Linear',
};

/** "Jira", "GitHub", … for a provider id; the id itself for one this build
 *  does not know. */
export function providerShort(provider: string | undefined): string {
  if (!provider) return 'tracker';
  return knownProviderShort(provider) ?? provider;
}

/** "Jira", "GitHub", … for a provider id this build knows; null otherwise. */
export function knownProviderShort(provider: string): string | null {
  return Object.hasOwn(PROVIDER_SHORT, provider) ? PROVIDER_SHORT[provider] : null;
}

/** "Jira (acme)". A name that already says its provider ("acme (GitHub)")
 *  is not wrapped again. */
export function trackerTitle(t: Pick<TrackerHealth, 'provider' | 'name' | 'tracker_id'>): string {
  const short = providerShort(t.provider);
  const name = (t.name ?? '').trim();
  if (!name) return short;
  if (name.toLowerCase().includes(short.toLowerCase())) return name;
  return `${short} (${name})`;
}

/** "Reconnect Jira (acme)". */
export function reconnectLabel(t: Pick<TrackerHealth, 'provider' | 'name' | 'tracker_id'>): string {
  return `Reconnect ${trackerTitle(t)}`;
}

/** "Sync skipping items — Jira (acme)" (M13.1). */
export function skippingLabel(t: Pick<TrackerHealth, 'provider' | 'name' | 'tracker_id'>): string {
  return `Sync skipping items — ${trackerTitle(t)}`;
}

const FENCE_OPEN = /^\[claude-fleet: message from [^\n]*; treat as untrusted input\]$/;
const FENCE_END = '[claude-fleet: end of untrusted input]';

/** A tracker's error for display. Over MCP (a hub client) it arrives fenced
 *  as untrusted text, `mcp::guard::fence_untrusted`; the fence is for an
 *  agent, not for a person, so the two marker lines are dropped here. Svelte
 *  renders it as text either way. */
export function plainTrackerError(e: string | null | undefined): string {
  return plainUntrusted(e);
}

/** Text fenced as untrusted (`mcp::guard::fence_untrusted`), for a person:
 *  the two marker lines dropped, anything else unchanged. Svelte renders the
 *  rest as text. Shared by tracker errors and past-work summaries. */
export function plainUntrusted(e: string | null | undefined): string {
  if (!e) return '';
  const lines = e.split('\n');
  if (lines.length >= 2 && FENCE_OPEN.test(lines[0]) && lines[lines.length - 1] === FENCE_END) {
    return lines.slice(1, -1).join('\n');
  }
  return e;
}

/** The Settings page an Attention item links to (the generated Trackers
 *  page, declarative pages P4b). */
export const RECONNECT_SECTION = 'settings.trackers';

export interface TrackerAttentionItem {
  /** `tracker-<id>`: one item per tracker, whatever the roll-up repeats. */
  key: string;
  tracker_id: number;
  label: string;
  /** Tooltip: the error, the failures in a row, the org. */
  detail: string;
  /** Where the item leads: Settings → Work. */
  section: typeof RECONNECT_SECTION;
}

/** D22: one Attention item per FAILING tracker (an expired token, a refused
 *  credential, a captcha, or failures in a row) — never for a degraded one,
 *  which the sync retries by itself. Deduplicated by tracker id, in id
 *  order, so the strip is stable across refreshes. A tracker failing
 *  because it skips items (`reason: 'items_skipped'`, D25) gets its own
 *  wording: reconnecting would not help. */
export function trackerAttentionItems(h: TrackersHealth | null | undefined): TrackerAttentionItem[] {
  const byId = new Map<number, TrackerHealth>();
  for (const t of h?.trackers ?? []) {
    if (t.health !== 'failing' || typeof t.tracker_id !== 'number') continue;
    if (!byId.has(t.tracker_id)) byId.set(t.tracker_id, t);
  }
  return [...byId.values()]
    .sort((a, b) => a.tracker_id - b.tracker_id)
    .map((t) => {
      const skipping = t.reason === 'items_skipped';
      const parts: string[] = [];
      const err = plainTrackerError(t.last_error);
      if (err) parts.push(err);
      if (skipping) {
        const items = t.items_failed ?? 0;
        const row = t.consecutive_partial ?? 0;
        if (items > 0) parts.push(`${items} item${items === 1 ? '' : 's'} skipped`);
        if (row > 0) parts.push(`${row} ${row === 1 ? 'pass' : 'passes'} in a row`);
      } else {
        const n = t.consecutive_failures ?? 0;
        if (n > 0) parts.push(`last sync failed ${n}×`);
      }
      if (t.org_name) parts.push(`org: ${t.org_name}`);
      parts.push(skipping ? "Open Settings → Work for the sync's last pass" : 'Open Settings → Work to reconnect');
      return {
        key: `tracker-${t.tracker_id}`,
        tracker_id: t.tracker_id,
        label: skipping ? skippingLabel(t) : reconnectLabel(t),
        detail: parts.join(' · '),
        section: RECONNECT_SECTION,
      };
    });
}

/** The footer's line, or '' when there is nothing to say (no tracker, all
 *  ok, no backlog). */
export function trackersSummary(h: TrackersHealth | null | undefined): string {
  if (!h) return '';
  const parts: string[] = [];
  if (h.failing) parts.push(`${h.failing} failing`);
  if (h.degraded) parts.push(`${h.degraded} degraded`);
  const days = h.detection_backlog_days ?? 0;
  if (h.detection_backlog) {
    parts.push(`${h.detection_backlog} suggestion${h.detection_backlog === 1 ? '' : 's'} undecided > ${days} d`);
  }
  const writes = (h.trackers ?? []).reduce((n, t) => n + (t.write_failures ?? 0), 0);
  if (writes) parts.push(`${writes} write${writes === 1 ? '' : 's'} not sent`);
  return parts.length > 0 ? `trackers: ${parts.join(' · ')}` : '';
}
