/**
 * Work graph usage (M13.2, decision D24): `work_admin { usage, days? }` on a
 * standalone desktop. Types mirror `service/work/usage.rs`; the text form
 * mirrors its `UsageSummary::lines`, so a Copy from here reads like
 * `fleet-hub work usage`. Counts and ids only. The command is local-only:
 * a desktop paired with a hub shows no Usage section.
 */

import { invokeCmd, type Result } from './result';

export interface UsageSummary {
  days: number;
  since: number;
  until: number;
  links?: { created?: number; by_source?: Record<string, number> };
  detection?: {
    suggested?: number;
    confirmed_by_person?: number;
    confirmed_by_agent?: number;
    promoted?: number;
    rejected?: number;
    withdrawn?: number;
    carried?: number;
    expired?: number;
    median_decision_secs?: number | null;
    nudges?: number;
  };
  handover?: { requested?: number; written?: number; missing?: number; send_failed?: number };
  resume?: { resumed?: number; with_brief?: number; without_brief?: number };
  journal?: {
    briefs_queued?: number;
    briefs_delivered?: number;
    compact_summaries?: number;
    summaries?: number;
    write_backs?: number;
  };
  tidy?: { applied?: number; kept?: number; auto_tidied?: number; auto_by_reason?: Record<string, number> };
  trackers?: { tracker_id: number; passes?: number; passes_failed?: number; items_failed?: number }[];
  unrecorded?: string[];
}

/** The windows the section offers, in days. */
export const USAGE_WINDOWS = [7, 30, 90, 365] as const;

export function workUsage(days: number): Promise<Result<UsageSummary>> {
  return invokeCmd<UsageSummary>('work_usage', { days });
}

/** "59 s", "10 min", "3 h", "5 d" (the Rust `duration`). */
export function usageDuration(secs: number): string {
  if (secs < 120) return `${secs} s`;
  if (secs < 7_200) return `${Math.floor(secs / 60)} min`;
  if (secs < 172_800) return `${Math.floor(secs / 3_600)} h`;
  return `${Math.floor(secs / 86_400)} d`;
}

function pairs(m: Record<string, number> | undefined): string {
  const keys = Object.keys(m ?? {}).sort();
  if (keys.length === 0) return 'none';
  return keys.map((k) => `${k} ${m![k]}`).join(', ');
}

/** One row per group: `[group, what it counted]`. */
export function usageRows(u: UsageSummary): [string, string][] {
  const n = (v: number | undefined) => v ?? 0;
  const d = u.detection ?? {};
  const h = u.handover ?? {};
  const r = u.resume ?? {};
  const j = u.journal ?? {};
  const t = u.tidy ?? {};
  const median = d.median_decision_secs == null ? 'n/a' : usageDuration(d.median_decision_secs);
  const rows: [string, string][] = [
    ['links', `${n(u.links?.created)} made (${pairs(u.links?.by_source)})`],
    [
      'detection',
      `${n(d.suggested)} suggested, ${n(d.confirmed_by_person)} confirmed by a person, ` +
        `${n(d.confirmed_by_agent)} confirmed by an agent, ${n(d.promoted)} promoted, ` +
        `${n(d.rejected)} rejected, ${n(d.withdrawn)} withdrawn, ${n(d.carried)} carried, ` +
        `${n(d.expired)} expired; ` +
        `median decision ${median}; ${n(d.nudges)} nudges`,
    ],
    [
      'handover',
      `${n(h.requested)} requested, ${n(h.written)} written, ${n(h.missing)} missing, ${n(h.send_failed)} send failed`,
    ],
    ['resume', `${n(r.resumed)} (${n(r.with_brief)} with a brief, ${n(r.without_brief)} without)`],
    [
      'journal',
      `${n(j.briefs_queued)} briefs queued, ${n(j.briefs_delivered)} delivered; ${n(j.compact_summaries)} compaction summaries, ` +
        `${n(j.summaries)} session summaries, ${n(j.write_backs)} PR links written`,
    ],
    ['tidy', `${n(t.applied)} applied, ${n(t.kept)} kept, ${n(t.auto_tidied)} auto-tidied (${pairs(t.auto_by_reason)})`],
  ];
  const trackers = u.trackers ?? [];
  if (trackers.length === 0) rows.push(['trackers', 'none']);
  for (const tr of trackers) {
    rows.push([
      `tracker ${tr.tracker_id}`,
      `${n(tr.passes)} passes, ${n(tr.passes_failed)} failed, ${n(tr.items_failed)} items skipped (since the sync started)`,
    ]);
  }
  return rows;
}

/** The plain-text form, line for line `fleet-hub work usage`'s. */
export function usageText(u: UsageSummary): string {
  return [
    `work graph usage, last ${u.days} d`,
    ...usageRows(u).map(([g, v]) => `${g}: ${v}`),
    ...(u.unrecorded ?? []).map((x) => `not recorded: ${x}`),
  ].join('\n');
}
