// The fleet's own software updates in fleet health (review w26-3), on the
// desktop.
//
// `fleet_health.updates` (`health_check` here; the hub's `fleet_health` when
// paired) carries what needs a person: an update that failed or was rolled
// back, a rollback that failed (that target is stopped), an update a target
// must take, a stale release channel, a paused rollout. Each raises ONE
// Attention item, linking to Settings → Updates.
import { writable } from 'svelte/store';

/** `service::update::UpdateAttention`. */
export interface UpdateAttention {
  /** `update_required` | `update_failed` | `update_rolled_back` |
   *  `rollback_failed` | `channel_stale` | `rollout_paused`. */
  reason: string;
  /** `client:3`, `agent:<host>`, `hub:self`, `channel:<track>`, … */
  target: string;
  component?: string;
  version?: string;
  detail?: string | null;
}

/** `service::update::UpdatesHealth`; absent from an older hub. */
export interface UpdatesHealth {
  /** `fresh`, `stale` or `none`. */
  channel?: string;
  attention?: UpdateAttention[];
}

/** The latest roll-up the desktop has read (null: none yet). */
export const updatesHealth = writable<UpdatesHealth | null>(null);

/** The Settings section the items open (`settings_tree.ts`). */
export const UPDATES_SECTION = 'updates';

export interface UpdateAttentionItem {
  key: string;
  label: string;
  detail: string;
  section: typeof UPDATES_SECTION;
}

/** "the hub", "agent on mercury", "device 3", the channel. */
function who(a: UpdateAttention): string {
  const [kind, rest] = a.target.split(/:(.*)/s);
  switch (kind) {
    case 'hub':
      return 'the hub';
    case 'agent':
      return `agent on ${rest}`;
    case 'client':
      return a.component === 'android' || a.component === 'ios' ? `phone ${rest}` : `device ${rest}`;
    default:
      return a.target;
  }
}

const LABEL: Record<string, (a: UpdateAttention) => string> = {
  rollback_failed: (a) =>
    a.target === 'hub:self' ? 'Hub update: rollback failed — hub stopped' : `Rollback failed — ${who(a)}`,
  update_failed: (a) => `Update failed — ${who(a)}`,
  update_rolled_back: (a) => `Update rolled back — ${who(a)}`,
  update_required: (a) => `Update required — ${who(a)}`,
  channel_stale: () => 'Release channel stale',
  rollout_paused: (a) => `Rollout paused — ${a.component || a.target}`,
};

/** PURE: one item per entry the hub flags, deduplicated by reason and
 *  target. A reason this build does not know still shows, in its own code. */
export function updateAttentionItems(h: UpdatesHealth | null | undefined): UpdateAttentionItem[] {
  const out: UpdateAttentionItem[] = [];
  const seen = new Set<string>();
  for (const a of h?.attention ?? []) {
    const key = `update:${a.reason}:${a.target}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const label = (LABEL[a.reason] ?? ((x: UpdateAttention) => `Update: ${x.reason.replace(/_/g, ' ')} — ${who(x)}`))(a);
    const parts = [a.version ? `${a.target} runs ${a.version}` : a.target];
    if (a.detail) parts.push(a.detail);
    parts.push('Open Settings → Updates');
    out.push({ key, label, detail: parts.join(' · '), section: UPDATES_SECTION });
  }
  return out;
}
