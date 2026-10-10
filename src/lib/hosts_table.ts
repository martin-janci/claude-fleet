// The Hosts page table (Orbit Fleet redesign step 4.6, Accounts board →
// "Hosts"): one row per host, flat rather than grouped by account, with the
// columns Host · Connection · Sessions · Disk · Agent · Accounts signed in.
// Pure, so the table and its tests agree; `HostsTable.svelte` renders.
import type { AccountRow } from "./accounts";
import { accountLabel } from "./accounts";
import {
  classify,
  NEEDS_YOU_COUNTED_BUCKETS,
  type AttentionOptions,
} from "./attention";
import { formatAge } from "./hook_health";
import type { HostRow } from "./hosts";
import { compareVersions } from "./hosts_view";
import type { SessionRow } from "./sessions";

const NEEDS_YOU = new Set(NEEDS_YOU_COUNTED_BUCKETS);

/** `local`, `SSH · 18 ms`, `agent · 40 ms`, `offline`. The latency is the
 *  probe's round trip of an empty command; absent until a probe timed one. */
export function connectionText(h: HostRow): string {
  if (h.alias === "local") return "local";
  if (!h.reachable) return "offline";
  const via = h.transport === "agent" ? "agent" : "SSH";
  return h.latency_ms != null ? `${via} · ${h.latency_ms} ms` : via;
}

export interface SessionsCell {
  total: number;
  needsYou: number;
  failed: number;
  /** `6 · 2 need you · 1 failed` (zero parts omitted). */
  text: string;
}

/** Sessions on the host, as the board counts them: the total, then how many
 *  need you (the Inbox's counted buckets) and how many failed. External rows
 *  (Claude running outside fleet) are not counted, as in the Hosts list. */
export function sessionsCell(
  alias: string,
  rows: readonly SessionRow[],
  opts: AttentionOptions,
): SessionsCell {
  let total = 0;
  let needsYou = 0;
  let failed = 0;
  for (const s of rows) {
    if (s.host_alias !== alias || s.kind === "external") continue;
    total++;
    const bucket = classify(s, opts);
    if (bucket === "failed" || bucket === "stop_failed") failed++;
    else if (NEEDS_YOU.has(bucket)) needsYou++;
  }
  const parts = [String(total)];
  if (needsYou) parts.push(`${needsYou} need${needsYou === 1 ? "s" : ""} you`);
  if (failed) parts.push(`${failed} failed`);
  return { total, needsYou, failed, text: parts.join(" · ") };
}

/** `412 GB`, `1.2 TB`, `840 MB` from kB, binary units like the rest of the Hosts view. */
export function sizeText(kb: number): string {
  const gb = kb / (1024 * 1024);
  if (gb >= 1024) {
    const tb = gb / 1024;
    return `${tb >= 10 ? Math.round(tb) : tb.toFixed(1)} TB`;
  }
  if (gb >= 1) return `${gb >= 10 ? Math.round(gb) : gb.toFixed(1)} GB`;
  return `${Math.round(kb / 1024)} MB`;
}

export interface DiskCell {
  /** `412 GB of 994 GB` used of total, or `—` before the first sample. */
  text: string;
  level: "ok" | "warn" | "crit" | null;
}

/** Used of total on `$HOME`'s filesystem; the levels match `diskMeter`. */
export function diskCell(h: HostRow): DiskCell {
  const free = h.disk_home_free_kb ?? null;
  const total = h.disk_home_total_kb ?? null;
  if (free === null || total === null || total <= 0)
    return { text: "—", level: null };
  const used = Math.max(0, total - free);
  const pct = Math.round((used * 100) / total);
  const level = pct >= 95 ? "crit" : pct >= 90 ? "warn" : "ok";
  return { text: `${sizeText(used)} of ${sizeText(total)}`, level };
}

export interface AgentCell {
  /** The Claude Code version, `—` when unknown. */
  text: string;
  /** Older than the newest fresh version in the fleet. */
  update: boolean;
}

export function agentCell(h: HostRow, newestClaude: string | null): AgentCell {
  const v = h.claude_version?.trim() || null;
  if (!v) return { text: "—", update: false };
  const text = /\d+(?:\.\d+)*/.exec(v)?.[0] ?? v;
  return {
    text,
    update: newestClaude !== null && compareVersions(v, newestClaude) < 0,
  };
}

/** The accounts signed in on the host: its own `/login`, then each login
 *  profile's, deduplicated, by short label (an email's local part). */
export function accountsSignedIn(
  h: HostRow,
  byUuid: ReadonlyMap<string, AccountRow>,
): string[] {
  const out: string[] = [];
  const add = (label: string | null | undefined) => {
    const l = label?.trim();
    if (!l) return;
    const short =
      l.includes("@") && !l.startsWith("@") ? l.slice(0, l.indexOf("@")) : l;
    if (!out.includes(short)) out.push(short);
  };
  if (h.account_uuid) {
    const a = byUuid.get(h.account_uuid);
    add(a ? accountLabel(a) : h.account_uuid.slice(0, 8));
  }
  for (const p of h.claude_profiles ?? []) {
    if (!p.account_uuid) continue;
    const a = byUuid.get(p.account_uuid);
    add(a ? accountLabel(a) : (p.email ?? p.account_uuid.slice(0, 8)));
  }
  return out;
}

/** The probe facts under a host's name: `16 CPU · 64 GB RAM · worktrees
 *  9.0 GB · booted 3d ago`, only the parts the host answered. */
export function machineLine(h: HostRow, now: number): string {
  const parts: string[] = [];
  if (h.cpu_count != null) parts.push(`${h.cpu_count} CPU`);
  if (h.mem_total_kb != null) parts.push(`${sizeText(h.mem_total_kb)} RAM`);
  if (h.worktree_kb != null) parts.push(`worktrees ${sizeText(h.worktree_kb)}`);
  if (h.boot_at != null && h.boot_at <= now) {
    const secs = now - h.boot_at;
    parts.push(
      `booted ${secs >= 86400 ? `${Math.floor(secs / 86400)}d` : formatAge(secs)} ago`,
    );
  }
  return parts.join(" · ");
}

/** Table order: `local` first, then reachable hosts, then by alias. */
export function tableOrder(hosts: readonly HostRow[]): HostRow[] {
  return [...hosts].sort((a, b) => {
    if ((a.alias === "local") !== (b.alias === "local"))
      return a.alias === "local" ? -1 : 1;
    if (a.reachable !== b.reachable) return a.reachable ? -1 : 1;
    return a.alias.localeCompare(b.alias);
  });
}

/** The Hosts view's Tidy hint (gap plan G4.5): the stopped sessions on one
 *  host whose worktrees a Tidy clean up would free, with their total size.
 *  Null when the tidy report names none there with a measured worktree. */
export interface HostTidyHint {
  sessionIds: number[];
  kb: number;
  text: string;
}

export function hostTidyHint(
  candidates: readonly { session_id: number; host_alias: string; worktree_kb?: number | null }[],
  alias: string,
): HostTidyHint | null {
  const here = candidates.filter((c) => c.host_alias === alias && c.worktree_kb != null && c.worktree_kb > 0);
  if (here.length === 0) return null;
  const kb = here.reduce((t, c) => t + (c.worktree_kb ?? 0), 0);
  const n = here.length;
  return {
    sessionIds: here.map((c) => c.session_id),
    kb,
    text: `${n} stopped session${n === 1 ? '' : 's'} on ${alias} hold${n === 1 ? 's' : ''} ${sizeText(kb)} of worktrees.`,
  };
}

/** "Move to host" facts (gap plan G4.5): free disk, load and whether the
 *  host's account is at its limit, for picking where a session goes.
 *  `limited` is `attentionFacts.limited_accounts`. Empty when nothing is
 *  known. */
export function moveTargetFacts(
  h: Pick<HostRow, 'disk_home_free_kb' | 'load_1m' | 'account_uuid'>,
  limited: Readonly<Record<string, { resets_at: number | null }>> | undefined,
  now: number,
): string {
  const parts: string[] = [];
  if (h.disk_home_free_kb != null) parts.push(`${sizeText(h.disk_home_free_kb)} free`);
  if (h.load_1m != null) parts.push(`load ${h.load_1m.toFixed(1)}`);
  const limit = h.account_uuid ? limited?.[h.account_uuid] : undefined;
  if (limit && (limit.resets_at == null || limit.resets_at > now)) parts.push('account at limit');
  return parts.join(' · ');
}
