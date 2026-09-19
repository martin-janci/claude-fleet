import { describe, it, expect, vi } from 'vitest';
import { onTimelineEvent, onConversationsChanged, dispatchTimelineEvents, dispatchConversationsChanged } from './live_events';

const ev = (id: number, session_id: number) =>
  ({ id, session_id, at: 1, kind: 'compact_done', detail: null, claude_session_id: 'a' });

describe('live_events', () => {
  it('delivers a timeline event only to its session and stops after unsubscribe', () => {
    const a = vi.fn(), b = vi.fn();
    const offA = onTimelineEvent(1, a);
    onTimelineEvent(2, b);
    dispatchTimelineEvents([ev(10, 1)]);
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).not.toHaveBeenCalled();
    offA();
    dispatchTimelineEvents([ev(11, 1)]);
    expect(a).toHaveBeenCalledTimes(1);
  });

  it('coalesces duplicate conversation-list changes per flush', () => {
    const f = vi.fn();
    onConversationsChanged(3, f);
    dispatchConversationsChanged([3, 3, 4]);
    expect(f).toHaveBeenCalledTimes(1);
  });
});
