// What a screen hands an `account_usage` item through its slot (declarative
// pages L8). Each slot promises part of it — `docs/pages.md` → *Embed pages*
// lists which — and the owner resolves it from what it already holds, so the
// item never looks up a host or account the screen did not show.
import type { AccountUsageSnapshot } from '../../account_usage_store';
import type { AccountRow } from '../../accounts';
import type { HostRow } from '../../hosts';

export interface UsageContext {
  /** Unix seconds; the owner ticks it. */
  now: number;
  locale?: string;
  timeZone?: string;
  /** The host (host_detail, new_session_chip, new_session_host). */
  host?: HostRow | null;
  /** The host's or group's account, and its snapshot. */
  account?: AccountRow | null;
  snapshot?: AccountUsageSnapshot | null;
  /** host_detail: the other hosts logged in to this account. */
  sharedWith?: string[];
  /** host_detail: the owner shows one outage banner for every account. */
  suppressUnavailable?: boolean;
  /** host_detail: the owner's floor-respecting refresh. */
  onrefresh?: () => void;
  /** Why a refresh is refused here (a hub client), else null. */
  refreshBlocked?: string | null;
  /** new_session_host, status_footer: every host. */
  hosts?: HostRow[];
  /** status_footer: every account and snapshot. */
  accounts?: AccountRow[];
  snapshots?: Record<string, AccountUsageSnapshot>;
  /** status_footer: open the Hosts view at a host. */
  onopenhost?: (alias: string | null) => void;
}
