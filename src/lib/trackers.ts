/**
 * Trackers and tracker items (work graph M3). Types mirror
 * `crates/fleet-core/src/store/trackers.rs` and `store/work.rs`; every field a
 * newer hub may add is optional here, and an unknown `state` reads as
 * "not ok".
 */

/** `trackers.state`. Anything else a newer hub sends is treated as not ok. */
export type TrackerState =
  | 'ok'
  | 'auth_failed'
  | 'rate_limited'
  | 'unreachable'
  | 'captcha'
  | 'unconfigured';

export interface TrackerConfig {
  account_id?: string | null;
  display_name?: string | null;
  tz?: string | null;
  key_prefixes?: string[];
  sprint_projects?: string[];
  sprint_field?: string | null;
}

/** A tracker as every read returns it: never a secret, only a hint. */
export interface TrackerRow {
  id: number;
  provider: string;
  name: string;
  instance_id?: string | null;
  site_url: string;
  transport?: string;
  config?: TrackerConfig;
  state: TrackerState | string;
  last_sync_at?: number | null;
  last_error?: string | null;
  created_at: number;
  has_credential?: boolean;
  credential_hint?: string | null;
  auth_kind?: string | null;
  username?: string | null;
}

/** A work item, local or from a tracker. */
export interface WorkItemRow {
  id: number;
  source: string;
  key?: string | null;
  title: string;
  url?: string | null;
  /** todo | in_progress | done */
  status_category: string;
  created_at: number;
  updated_at: number;
  tracker_id?: number | null;
  external_id?: string | null;
  aliases?: string[];
  kind?: string | null;
  hierarchy_level?: number | null;
  status_name?: string | null;
  /** completed | not_planned | duplicate */
  resolution?: string | null;
  parent_id?: number | null;
  assignees?: string[];
  iteration?: string | null;
  updated_ext?: number | null;
  status_changed_at?: number | null;
  fetched_at?: number | null;
  unavailable_at?: number | null;
  unavailable_reason?: string | null;
}

/** One `work:*` frame, as the batched handler receives it. */
export type WorkEvent =
  | { type: 'item'; row: WorkItemRow }
  | { type: 'tracker'; row: TrackerRow }
  | { type: 'tracker_removed'; id: number };
