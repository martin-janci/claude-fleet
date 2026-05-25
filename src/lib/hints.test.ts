import { describe, it, expect } from 'vitest';
import { pickActiveHint, hintsGateOpen, HINTS, type HintId } from './hints';

const ORDER: HintId[] = HINTS.map((h) => h.id);

describe('pickActiveHint', () => {
  it('returns null when hints are disabled', () => {
    expect(pickActiveHint(ORDER, new Set(['host-filter']), [], false, true)).toBeNull();
  });

  it('returns null before the welcome is dismissed', () => {
    expect(pickActiveHint(ORDER, new Set(['host-filter']), [], true, false)).toBeNull();
  });

  it('returns null when nothing is registered', () => {
    expect(pickActiveHint(ORDER, new Set(), [], true, true)).toBeNull();
  });

  it('picks the first registered, unseen hint in priority order', () => {
    const reg = new Set<HintId>(['recency-filter', 'bg-session']);
    expect(pickActiveHint(ORDER, reg, [], true, true)).toBe('bg-session');
  });

  it('skips seen hints', () => {
    const reg = new Set<HintId>(['bg-session', 'session-actions']);
    expect(pickActiveHint(ORDER, reg, ['bg-session'], true, true)).toBe('session-actions');
  });

  it('returns null when all registered hints are seen', () => {
    const reg = new Set<HintId>(['host-filter']);
    expect(pickActiveHint(ORDER, reg, ['host-filter'], true, true)).toBeNull();
  });
});

describe('hintsGateOpen', () => {
  it('open when welcomed', () => {
    expect(hintsGateOpen(true, 0, 0)).toBe(true);
  });
  it('open when not welcomed but a host exists (existing user)', () => {
    expect(hintsGateOpen(false, 1, 0)).toBe(true);
  });
  it('open when not welcomed but a work session exists', () => {
    expect(hintsGateOpen(false, 0, 1)).toBe(true);
  });
  it('closed when not welcomed and the fleet is empty', () => {
    expect(hintsGateOpen(false, 0, 0)).toBe(false);
  });
});
