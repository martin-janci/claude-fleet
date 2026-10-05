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

  // Multi-user M1: a session reached through a grant HAS a pane, but not one
  // this client may attach — the terminal slot shows a read-only snapshot of
  // it instead, which needs no transcript. The missing-transcript fallback
  // would otherwise FORCE `terminal` on exactly those rows, which is the one
  // view the watcher did not ask for; hence the check sits before it.
  it('does not force Terminal on a session this client may not attach', () => {
    expect(resolveSessionView('conversation', false, false, false)).toBe('conversation');
    expect(resolveSessionView('terminal', false, false, false)).toBe('terminal');
  });

  it('leaves both views reachable for a watcher, so the snapshot is not stranded', () => {
    expect(resolveSessionView('terminal', false, true, false)).toBe('terminal');
    expect(resolveSessionView('conversation', false, true, false)).toBe('conversation');
  });

  it('still prefers Conversation for a pane-less row the client cannot attach', () => {
    expect(resolveSessionView('terminal', true, true, false)).toBe('conversation');
  });

  it('defaults to attachable, so every existing call site is unchanged', () => {
    expect(resolveSessionView('conversation', false, false)).toBe(
      resolveSessionView('conversation', false, false, true),
    );
  });
});

describe('otherSessionView', () => {
  it('flips between the two views', () => {
    expect(otherSessionView('conversation')).toBe('terminal');
    expect(otherSessionView('terminal')).toBe('conversation');
  });
});
