import { describe, it, expect } from 'vitest';
import { waitingForYou } from './waiting_count';
import { session } from './hosts_fixture';

const opts = { idleSecs: 0, now: 1_000_000 };
const blocked = (host: string, name: string, over = {}) =>
  session(host, name, { claude_status: 'blocked', ...over });

// Redesign step 3.14: the Halo's count is the Sidebar's Needs you count.
describe('waitingForYou', () => {
  it('counts the sessions waiting on a person', () => {
    expect(waitingForYou([blocked('trn', 'a'), session('trn', 'b'), blocked('nas', 'c')], opts)).toBe(2);
    expect(waitingForYou([session('trn', 'b')], opts)).toBe(0);
  });

  it('leaves out a mass loss folded into one row, as the Sidebar does', () => {
    const lost = (n: string) =>
      blocked('trn', n, { lost_at: 1, claude_session_id: `c-${n}`, kind: 'work' });
    expect(waitingForYou([lost('a'), lost('b'), lost('c'), blocked('nas', 'd')], opts)).toBe(1);
    // Two lost rows are not a fold, and still count.
    expect(waitingForYou([lost('a'), lost('b')], opts)).toBe(2);
  });
});
