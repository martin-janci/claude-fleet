import { describe, it, expect } from 'vitest';
import { rowElapsed, rowPrompt } from './session_status';
import type { SessionRow } from './sessions';

const base = { started_at: null, last_prompt: null, last_turn_at: null } as unknown as SessionRow;

describe('row text helpers', () => {
  it('rowElapsed is empty without a start and formats h/m otherwise', () => {
    expect(rowElapsed(base, 1000)).toBe('');
    expect(rowElapsed({ ...base, started_at: 1000 - 3 * 3600 - 5 * 60 }, 1000)).toBe('3h 5m');
  });
  it('rowPrompt takes the first line, truncated', () => {
    expect(rowPrompt(base)).toBe('');
    expect(rowPrompt({ ...base, last_prompt: 'Implement the triage filter\nsecond line' })).toBe('Implement the triage filter');
  });
});
