// Work graph M12.4: trackers in fleet health, on the desktop.
//
// `fleet_health` (`health_check` here; the hub's `fleet_health` when paired)
// carries a `trackers` roll-up read from the sync's in-memory metrics and the
// store — never a live call. This module turns it into:
//
// - the footer's one-line summary ("trackers: 1 failing · 3 undecided > 7 d");
// - decision D22's Attention items: ONE per failing tracker, deduplicated by
//   tracker id, "Reconnect Jira (acme)", linking to Settings → Work.
//
// Pure helpers plus one store; `TrackerAttention.svelte` polls and renders.
import { writable } from 'svelte/store';
import { healthCheck } from './ipc';

export type TrackerHealthLevel = 'ok' | 'degraded' | 'failing';

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
  last_error?: string | null;
  last_success_at?: number | null;
  last_pass_at?: number | null;
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
  if (r.ok) trackersHealth.set(r.value.trackers ?? null);
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
  return PROVIDER_SHORT[provider] ?? provider;
}

/** "Reconnect Jira (acme)". A name that already says its provider ("acme
 *  (GitHub)") is not wrapped again. */
export function reconnectLabel(t: Pick<TrackerHealth, 'provider' | 'name' | 'tracker_id'>): string {
  const short = providerShort(t.provider);
  const name = (t.name ?? '').trim();
  if (!name) return `Reconnect ${short}`;
  if (name.toLowerCase().includes(short.toLowerCase())) return `Reconnect ${name}`;
  return `Reconnect ${short} (${name})`;
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

/** The Settings section an Attention item links to. */
export const RECONNECT_SECTION = 'work';

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
 *  order, so the strip is stable across refreshes. */
export function trackerAttentionItems(h: TrackersHealth | null | undefined): TrackerAttentionItem[] {
  const byId = new Map<number, TrackerHealth>();
  for (const t of h?.trackers ?? []) {
    if (t.health !== 'failing' || typeof t.tracker_id !== 'number') continue;
    if (!byId.has(t.tracker_id)) byId.set(t.tracker_id, t);
  }
  return [...byId.values()]
    .sort((a, b) => a.tracker_id - b.tracker_id)
    .map((t) => {
      const parts: string[] = [];
      const err = plainTrackerError(t.last_error);
      if (err) parts.push(err);
      const n = t.consecutive_failures ?? 0;
      if (n > 0) parts.push(`last sync failed ${n}×`);
      if (t.org_name) parts.push(`org: ${t.org_name}`);
      parts.push('Open Settings → Work to reconnect');
      return {
        key: `tracker-${t.tracker_id}`,
        tracker_id: t.tracker_id,
        label: reconnectLabel(t),
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
  return parts.length > 0 ? `trackers: ${parts.join(' · ')}` : '';
}
