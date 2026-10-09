import { render, screen } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import SessionRowItem from './SessionRowItem.svelte';
import { hosts } from './hosts';
import { session } from './hosts_fixture';
import { hubStatus, STANDALONE } from './hub';
import { resetAccessForTests } from './access';
import { showRowDetails, type SessionRow } from './sessions';
import { COMPACT_ROW_PX, uiDensity } from './prefs';
import { accounts } from './accounts';
import { ADMIN, fleetAccounts } from './hosts_fixture';

// Redesign step 3.6: the density setting. Comfortable is 0.5.4's row with
// every badge; Compact is the two-line row (sans title, one meta line, chips
// on hover) at a fixed height.

const noop = () => {};
const NOW = Math.floor(Date.now() / 1000);

// One row carrying every badge 0.5.4 drew.
const full: SessionRow = session('mac', 'fix-hub-e2e', {
  claude_status: 'working',
  current_activity: 'Running tests',
  started_at: NOW - 3600,
  last_activity_at: NOW - 120,
  context_pct: 55,
  usage_input_tokens: 1000,
  usage_cost_micros: 8_510_000,
  effort_level: 'high',
  pr_url: 'https://github.com/o/r/pull/476',
  ci_status: 'passing',
  pr_checked_at: NOW,
  last_prompt: 'Fix the flake',
  visibility: 'private',
  owner_person_id: 1,
  worktree_key: 'wt',
  kind: 'review',
} as Partial<SessionRow>);

function props(sess: SessionRow) {
  return {
    sess,
    selectMode: false,
    isChecked: false,
    isRenaming: false,
    renameValue: '',
    renameInput: undefined,
    renameError: null,
    relatedCount: 2,
    nowSec: NOW,
    onSelectSession: noop,
    onKeySession: noop,
    toggleSelected: noop,
    beginRename: noop,
    beginLabelEdit: noop,
    onRenameKey: noop,
    commitRename: noop,
    askRecreate: noop,
    askRestart: noop,
    askKill: noop,
  };
}

const BADGES_054 = [
  'related-badge',
  'privacy-chip',
  'claude-chip',
  'host-badge',
  'sess-tmux-name',
  'context-badge',
  'cost-badge',
  'ci-badge',
  'sess-meta',
];

beforeEach(() => {
  uiDensity.set('comfortable');
  hosts.set([]);
  hubStatus.set({ ...STANDALONE });
  resetAccessForTests();
  showRowDetails.set(true);
});
afterEach(() => uiDensity.set('comfortable'));

describe('row density (redesign step 3.6)', () => {
  it('Comfortable shows every 0.5.4 badge, at no fixed height', async () => {
    render(SessionRowItem, { props: props(full) });
    await tick();
    const row = screen.getByTestId('sess-row');
    expect(row.dataset.density).toBe('comfortable');
    expect(row.classList.contains('compact')).toBe(false);
    expect(row.style.minHeight).toBe('');
    for (const id of BADGES_054) expect(screen.queryByTestId(id), id).not.toBeNull();
    expect(screen.getByText('high')).toBeTruthy();
    expect(screen.getByText('PR↗')).toBeTruthy();
    expect(screen.queryByTestId('sess-meta-line')).toBeNull();
    expect(screen.queryByTestId('sess-age')).toBeNull();
  });

  it('Compact draws two lines: name and age, then one meta line', async () => {
    uiDensity.set('compact');
    render(SessionRowItem, { props: props(full) });
    await tick();
    const row = screen.getByTestId('sess-row');
    expect(row.dataset.density).toBe('compact');
    expect(row.classList.contains('compact')).toBe(true);
    expect(row.style.minHeight).toBe(`${COMPACT_ROW_PX}px`);
    expect(screen.getByTestId('sess-age').textContent).toBe('2m');
    const meta = screen.getByTestId('sess-meta-line');
    expect(meta.textContent?.replace(/\s+/g, ' ').trim()).toBe('Working · Claude Code · mac · Running tests');
    // The badge line gives way to the meta line ...
    expect(screen.queryByTestId('sess-details')).toBeNull();
    // ... and the chips stay in the row, hidden by CSS until hover or focus.
    const chips = screen.getAllByTestId('row-chips');
    expect(chips.some((c) => c.querySelector('[data-testid="privacy-chip"]'))).toBe(true);
    expect(chips.some((c) => c.querySelector('[data-testid="claude-chip"]'))).toBe(true);
  });

  it('a row that needs you says so first on its meta line', async () => {
    uiDensity.set('compact');
    render(SessionRowItem, {
      props: props({ ...full, claude_status: 'blocked', current_activity: null, pending_input: null } as SessionRow),
    });
    await tick();
    const meta = screen.getByTestId('sess-meta-line');
    expect(meta.dataset.state).toBe('action_required');
    expect(meta.textContent).toContain('Needs you');
  });

  it('20 Compact rows fit a 1080p window', () => {
    // 1080 px less the title bar, header, filters and status bar the shell
    // draws around the list (280 px, generous for every layout).
    const chrome = 280;
    expect(20 * COMPACT_ROW_PX).toBeLessThanOrEqual(1080 - chrome);
  });
});

// Parity P15: density, not layout, decides the badges. New keeps every 0.5.4
// badge on a Comfortable row and adds the account pill (redesign step 4.3).
describe('row density in the New layout', () => {
  beforeEach(() => {
    accounts.set(fleetAccounts());
  });
  afterEach(() => {
    accounts.set([]);
  });

  it('New layout: Comfortable shows every 0.5.4 badge, plus the account pill', async () => {
    render(SessionRowItem, { props: props({ ...full, account_uuid: ADMIN.uuid }) });
    await tick();
    const row = screen.getByTestId('sess-row');
    expect(row.dataset.density).toBe('comfortable');
    expect(row.classList.contains('compact')).toBe(false);
    expect(row.style.minHeight).toBe('');
    for (const id of BADGES_054) expect(screen.queryByTestId(id), id).not.toBeNull();
    expect(screen.getByText('high')).toBeTruthy();
    expect(screen.getByText('PR↗')).toBeTruthy();
    expect(screen.queryByTestId('sess-meta-line')).toBeNull();
    expect(screen.queryByTestId('sess-age')).toBeNull();
    expect(screen.getByTestId('account-pill')).toBeTruthy();
  });
});
