// Redesign step 5.1: the agent tab is named and marked from the session's
// agent; ⌘J stays its chord (App.test.ts › App: the Conversation tab).
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import SessionTabs from './SessionTabs.svelte';
import { session } from './hosts_fixture';
import type { SessionRow } from './sessions';

function mount(row: SessionRow | null, isMac = true) {
  return render(SessionTabs, {
    props: {
      session: row,
      name: row?.tmux_name ?? '',
      current: row ? 'agent' : null,
      disabled: {},
      assetsActive: false,
      inspectorOpen: false,
      inspectorAvailable: true,
      isMac,
      onselect: () => {},
      onassets: () => {},
      oninspector: () => {},
    },
  });
}

describe('SessionTabs: the agent tab (step 5.1)', () => {
  it('a Claude session gets "Claude Code" with its orange mark and ⌘J', () => {
    mount(session('mercury', 'dev-a'));
    const tab = screen.getByTestId('stab-agent');
    expect(tab.textContent).toContain('Claude Code');
    const mark = tab.querySelector('svg.agent-mark');
    expect(mark?.getAttribute('data-agent')).toBe('claude');
    expect(tab.querySelector('.of-kbd')?.textContent).toBe('⌘J');
    expect(tab.getAttribute('title')).toBe('Claude Code (⌘J)');
  });

  it('another agent is named for itself and drawn with the terminal glyph', () => {
    mount(session('mercury', 'dev-b', { agent: 'codex' } as Partial<SessionRow>), false);
    const tab = screen.getByTestId('stab-agent');
    expect(tab.textContent).toContain('Codex');
    expect(tab.querySelector('svg.agent-mark')?.getAttribute('data-agent')).toBe('codex');
    expect(tab.querySelector('.of-kbd')?.textContent).toBe('Ctrl+Shift+J');
  });

  it('a shell session is "Shell"; no session shows the plain Terminal tab', () => {
    const { unmount } = mount(session('mercury', 'dev-c', { kind: 'shell' }));
    expect(screen.getByTestId('stab-agent').textContent).toContain('Shell');
    unmount();
    mount(null);
    const tab = screen.getByTestId('stab-agent');
    expect(tab.textContent).toContain('Terminal');
    expect(tab.querySelector('svg.agent-mark')).toBeNull();
  });
});

describe('SessionTabs: Share from the header (step 5.8)', () => {
  it('opens the one Share sheet on the session; disabled with no session', async () => {
    const { shareSheetFor } = await import('./share');
    shareSheetFor.set(null);
    const row = session('mercury', 'dev-s');
    const { unmount } = mount(row);
    const btn = screen.getByTestId('share-from-header') as HTMLButtonElement;
    expect(btn.disabled).toBe(false);
    btn.click();
    const { get } = await import('svelte/store');
    expect(get(shareSheetFor)).toBe(row.id);
    shareSheetFor.set(null);
    unmount();
  });
});
