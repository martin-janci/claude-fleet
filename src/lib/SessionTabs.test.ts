// Redesign step 5.1: the agent tab is named and marked from the session's
// agent; ⌘J stays its chord (App.test.ts › App: the Conversation tab).
import { render, screen } from '@testing-library/svelte';
import { describe, it, expect } from 'vitest';
import SessionTabs from './SessionTabs.svelte';
import { session } from './hosts_fixture';
import { expectAccessible } from './a11y_check';
import type { SessionRow } from './sessions';

function mount(row: SessionRow | null, isMac = true) {
  return render(SessionTabs, {
    props: {
      session: row,
      name: row?.tmux_name ?? '',
      current: row ? 'agent' : null,
      disabled: {},
      inspectorOpen: false,
      inspectorAvailable: true,
      isMac,
      onselect: () => {},
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

describe('SessionTabs: accessibility', () => {
  it('the session tab bar is accessible', async () => {
    const { container } = mount(session('mercury', 'dev-a'));
    await expectAccessible(container);
  });
});

describe('SessionTabs: the Terminals tab (step 5.3)', () => {
  it('sits after the agent tab with its count and the new-terminal chord', () => {
    render(SessionTabs, {
      props: {
        session: session('mercury', 'dev-a'),
        name: 'dev-a',
        current: 'terminals',
        terminalCount: 2,
        disabled: {},
        inspectorOpen: false,
        inspectorAvailable: true,
        isMac: true,
        onselect: () => {},
        oninspector: () => {},
      },
    });
    const ids = screen.getAllByRole('tab').map((t) => t.dataset.testid);
    expect(ids).toEqual(['stab-conversation', 'stab-agent', 'stab-terminals', 'stab-files', 'stab-details']);
    const tab = screen.getByTestId('stab-terminals');
    expect(tab.getAttribute('aria-selected')).toBe('true');
    expect(screen.getByTestId('stab-terminals-count').textContent).toBe('2');
    expect(tab.querySelector('.of-kbd')?.textContent).toBe('⌥⌘T');
  });

  it('shows no count with no terminals open, and says why it is off', () => {
    render(SessionTabs, {
      props: {
        session: session('mercury', 'dev-a'),
        name: 'dev-a',
        current: 'agent',
        disabled: { terminals: 'Terminals open only on a session that is yours' },
        inspectorOpen: false,
        inspectorAvailable: true,
        isMac: false,
        onselect: () => {},
        oninspector: () => {},
      },
    });
    const tab = screen.getByTestId('stab-terminals') as HTMLButtonElement;
    expect(screen.queryByTestId('stab-terminals-count')).toBeNull();
    expect(tab.disabled).toBe(true);
    expect(tab.title).toBe('Terminals open only on a session that is yours');
  });
});

describe('SessionTabs: the header per the Main board (UX audit 2026-10-09, H1–H3)', () => {
  it('names its buttons and carries the context meter and the worktree in the meta line', () => {
    const row = { ...session('mercury', 'dev-h'), context_pct: 55, context_tokens: 110_000, context_window: 200_000, worktree_key: 'fix-flake' };
    mount(row);
    expect(screen.getByTestId('open-in-editor').textContent).toContain('Open in VS Code');
    expect(screen.getByTestId('share-from-header').textContent).toContain('Share…');
    const ctx = screen.getByTestId('session-head-context');
    expect(ctx.textContent).toContain('55%');
    expect(ctx.textContent).toContain('of 200k');
    expect(ctx.querySelector('[role="meter"]')).not.toBeNull();
    expect(screen.getByTestId('session-head-worktree').textContent).toBe('fix-flake');
  });

  it('shows no meter when the context is unknown', () => {
    mount({ ...session('mercury', 'dev-n'), context_pct: null });
    expect(screen.queryByTestId('session-head-context')).toBeNull();
  });
});
