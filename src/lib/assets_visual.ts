// Assets M5: the visual vocabulary the workspace shares — badge tones and
// host-dot states (spec, Visuals; Rulings R26, R27). Colour reinforces a
// state; a word or a shape always carries it.

export type BadgeTone = 'neutral' | 'ok' | 'warn' | 'crit' | 'accent' | 'muted';

/** One host's dot in a `HostStrip`. */
export type DotState = 'present' | 'in_sync' | 'differs' | 'missing' | 'absent' | 'na' | 'stale' | 'blocked';

/** What each dot says to a screen reader and in its tooltip. */
export const DOT_LABEL: Record<DotState, string> = {
  present: 'present',
  in_sync: 'in sync',
  differs: 'differs',
  missing: 'missing',
  absent: 'absent',
  na: 'not here',
  stale: 'stale scan',
  blocked: 'blocked',
};

/** A sync op's badge tone, by what it does to a host. */
export function opTone(op: string): BadgeTone {
  if (op === 'overwrite' || op === 'remove') return 'crit';
  if (op === 'create' || op === 'adopt' || op === 'plugin_install') return 'ok';
  if (op === 'update' || op === 'plugin_update') return 'warn';
  return 'muted';
}

/** An applied action's outcome tone. */
export function outcomeTone(outcome: string): BadgeTone {
  if (outcome === 'done') return 'ok';
  if (outcome === 'skipped') return 'muted';
  return 'crit';
}
