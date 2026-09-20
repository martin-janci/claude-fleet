import { describe, it, expect } from 'vitest';
import { resolveSessionView, otherSessionView } from './session_view';

describe('resolveSessionView', () => {
  it('honours the stored preference when the row can show either view', () => {
    expect(resolveSessionView('conversation', false, true)).toBe('conversation');
    expect(resolveSessionView('terminal', false, true)).toBe('terminal');
  });

  it('forces Conversation on a row with no tmux pane — there is no PTY to show', () => {
    expect(resolveSessionView('terminal', true, true)).toBe('conversation');
    expect(resolveSessionView('conversation', true, true)).toBe('conversation');
  });

  it('forces Terminal on a row with no claude_session_id — there is no transcript yet', () => {
    expect(resolveSessionView('conversation', false, false)).toBe('terminal');
    expect(resolveSessionView('terminal', false, false)).toBe('terminal');
  });

  it('prefers Conversation when a row is both pane-less and id-less', () => {
    expect(resolveSessionView('terminal', true, false)).toBe('conversation');
    expect(resolveSessionView('conversation', true, false)).toBe('conversation');
  });
});

describe('otherSessionView', () => {
  it('flips between the two views', () => {
    expect(otherSessionView('conversation')).toBe('terminal');
    expect(otherSessionView('terminal')).toBe('conversation');
  });
});
