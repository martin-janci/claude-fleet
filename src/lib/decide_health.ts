// The decision envelope's health (Jev, test map §7), on the desktop.
//
// `fleet_health.decide` (`health_check` here; the hub's `fleet_health` when
// paired) says whether the live decision calls of the last hour are
// failing. Answers fall back to what fleet does today by themselves; a
// degraded envelope raises ONE Attention item so a person looks, linking to
// Settings → Decisions (Jev). Absent when nothing is on, and for a scoped
// caller.
import { writable } from 'svelte/store';

/** `service::decide::DecideHealth`. Null-stripped on the hub's wire. */
export interface DecideHealth {
  enabled?: boolean;
  /** Feature → `shadow` | `assist`, for every feature that is on. */
  modes?: Record<string, string>;
  window_secs?: number;
  attempts?: number;
  failures?: number;
  failure_rate?: number | null;
  breaker_open?: boolean;
  budget_spent?: boolean;
  degraded?: boolean;
  /** `breaker_open` | `failure_rate` when degraded. */
  reason?: string | null;
}

/** The latest one the desktop has read (null: none, or nothing is on). */
export const decideHealth = writable<DecideHealth | null>(null);

/** The Settings section the item opens (`[data-testid="decide-section"]`). */
export const DECIDE_SECTION = 'decide';

export interface DecideAttentionItem {
  key: 'decide';
  label: string;
  /** Tooltip: why, in numbers. */
  detail: string;
  section: typeof DECIDE_SECTION;
}

/** One item when the envelope is degraded, else none. */
export function decideAttentionItem(h: DecideHealth | null | undefined): DecideAttentionItem | null {
  if (!h?.degraded) return null;
  const parts: string[] = [];
  if (h.reason === 'breaker_open' || h.breaker_open) {
    parts.push('the circuit breaker is open');
  }
  const attempts = h.attempts ?? 0;
  if (attempts > 0) {
    const pct = typeof h.failure_rate === 'number' ? ` (${Math.round(h.failure_rate * 100)}%)` : '';
    const mins = Math.round((h.window_secs ?? 3600) / 60);
    parts.push(`${h.failures ?? 0} of ${attempts} calls failed in the last ${mins} min${pct}`);
  }
  parts.push("answers fall back to today's rules by themselves");
  parts.push('Open Settings → Decisions (Jev)');
  return {
    key: 'decide',
    label: 'Jev degraded',
    detail: parts.join(' · '),
    section: DECIDE_SECTION,
  };
}
