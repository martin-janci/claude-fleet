// The folded sibling strip says what it hides that wants a look: its
// waiting and failed sessions (review 2026-11-10).
import { render } from '@testing-library/svelte';
import { describe, it, expect, afterEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => { throw { code: 'E_TEST', message: 'no backend' }; }) }));
import SessionFocus from './SessionFocus.svelte';
import { session } from './hosts_fixture';
import { siblingsFold } from './prefs';

afterEach(() => siblingsFold.set('auto'));

describe('SessionFocus', () => {
  const strip = () => [
    session('mercury', 'a', { claude_status: 'working' }),
    session('mercury', 'b', { claude_status: 'blocked' }),
    session('mercury', 'c', { claude_status: 'failed' }),
    session('mercury', 'd', { claude_status: 'blocked' }),
  ];

  it('names the waiting and failed siblings on the folded toggle only', () => {
    siblingsFold.set('closed');
    const sibs = strip();
    const props = { session: sibs[0], siblings: sibs, siblingsTitle: 'In this mission', onpick: () => {}, onfull: () => {} };
    const { getByTestId, unmount } = render(SessionFocus, props);
    expect(getByTestId('control-session-siblings-attention').textContent).toBe('· 2 waiting · 1 failed');
    unmount();
    siblingsFold.set('open');
    const again = render(SessionFocus, props);
    expect(again.getAllByTestId('control-session-sibling')).toHaveLength(4);
    expect(again.queryByTestId('control-session-siblings-attention')).toBeNull();
  });

  it('says nothing more when no sibling wants a look', () => {
    siblingsFold.set('closed');
    const sibs = [session('mercury', 'a', { claude_status: 'working' }), session('mercury', 'b', { claude_status: 'idle' })];
    const { queryByTestId } = render(SessionFocus, { session: sibs[0], siblings: sibs, onpick: () => {}, onfull: () => {} });
    expect(queryByTestId('control-session-siblings-attention')).toBeNull();
  });
});
