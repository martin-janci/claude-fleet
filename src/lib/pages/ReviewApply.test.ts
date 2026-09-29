// Declarative pages P5: an agent's settings proposals, reviewed. The review
// page (layout L6) lists them grouped by where each setting lives; a field
// shows its own proposal inline as a suggestion; nothing is written until a
// person applies it (`decide_setting_proposals`). History reads the audit.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import PageView from './PageView.svelte';
import { fleetSettings, SETTING_DEFAULTS } from '../fleet_settings';
import { toasts } from '../toasts';
import { allDescriptors, bundle, registryRouter } from './testing';
import { homeOf, type Page } from './pages';
import { settingProposals, type SettingProposal } from './review';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const descs = new Map(allDescriptors.map((d) => [d.key, d]));
const defaults = Object.fromEntries(allDescriptors.map((d) => [d.key, d.value]));
const pageOf = (id: string) => bundle.pages.find((p) => p.id === id) as Page;
const NOW = 1_800_000_000;

const proposal = (over: Partial<SettingProposal> = {}): SettingProposal => ({
  id: 1,
  at: NOW - 120,
  key: 'work.recent_days',
  value: '3',
  before: defaults['work.recent_days'],
  current: defaults['work.recent_days'],
  why: 'a shorter Recent list',
  source: 'agent',
  source_detail: 'control API',
  state: 'pending',
  ...over,
});

function show(id: string, proposals: SettingProposal[], readonly = false) {
  const onopen = vi.fn();
  render(PageView, {
    props: {
      page: pageOf(id),
      pages: bundle.pages,
      descs,
      values: defaults,
      sources: bundle.sources,
      proposals,
      readonly,
      onnavigate: () => {},
      onopen,
    },
  });
  return onopen;
}

let decided: { accept: number[]; reject: number[] }[] = [];

beforeEach(() => {
  decided = [];
  toasts.set([]);
  inv.mockReset();
  inv.mockImplementation(
    registryRouter({}, (cmd, args) => {
      if (cmd === 'decide_setting_proposals') {
        const a = args as { accept: number[]; reject: number[] };
        decided.push(a);
        return { applied: a.accept.filter((id) => id !== 99), rejected: a.reject, failed: a.accept.includes(99) ? [{ id: 99, error: 'no longer waiting for review' }] : [] };
      }
      if (cmd === 'setting_proposals') return { can_write: true, proposals: [] };
      if (cmd === 'setting_history')
        return [
          { id: 2, at: NOW - 60, key: 'work.recent_days', before: '14', after: '3', actor: 'person', proposal_id: 1 },
          { id: 1, at: NOW - 3600, key: 'work.recent_days', before: null, after: '14', actor: 'agent', actor_detail: 'control API' },
        ];
      return null;
    }).impl,
  );
});
afterEach(() => {
  fleetSettings.set({ ...SETTING_DEFAULTS });
  settingProposals.set([]);
});

describe('the review page (review_apply)', () => {
  it('groups proposals by the page each setting lives on, as now → proposed, with who and why', () => {
    show('settings.review', [proposal(), proposal({ id: 2, key: 'playbooks.press_enter', value: 'true', before: 'false', current: 'false', why: undefined })]);
    const home = pageOf(homeOf(bundle.pages, 'work.recent_days')!.page).title;
    expect(screen.getByTestId(`review-group-${home}`)).toBeInTheDocument();
    const diff = screen.getByTestId('review-diff-work.recent_days').textContent!;
    expect(diff).toMatch(new RegExp(`${defaults['work.recent_days']} days\\s*→\\s*3 days`));
    expect(screen.getByTestId('review-diff-playbooks.press_enter').textContent).toMatch(/Off\s*→\s*On/);
    expect(screen.getByTestId('review-row-work.recent_days').textContent).toContain('a shorter Recent list');
    expect(screen.getByTestId('review-row-work.recent_days').textContent).toContain('an agent (control API)');
    expect(screen.getByTestId('review-apply-selected').textContent).toContain('(2)');
  });

  it('leaves a value that moved since it was proposed unticked, and applies only what is ticked', async () => {
    show('settings.review', [
      proposal(),
      proposal({ id: 2, key: 'playbooks.press_enter', value: 'true', before: 'false', current: 'false' }),
      proposal({ id: 3, key: 'usage.enabled', value: 'false', before: 'true', current: 'false' }),
      proposal({ id: 4, key: 'work.tidy_done_days', value: '5', before: '7', current: '9' }),
    ]);
    expect(screen.getByTestId('review-moved-work.tidy_done_days').textContent).toContain('it was 7 days');
    expect((screen.getByTestId('review-tick-work.tidy_done_days') as HTMLInputElement).checked).toBe(false);
    expect(screen.getByTestId('review-moved-usage.enabled').textContent).toContain('Already set');
    // playbooks.press_enter has not moved; shorten the list to the moved case:
    expect(screen.queryByTestId('review-moved-playbooks.press_enter')).toBeNull();
    expect((screen.getByTestId('review-tick-usage.enabled') as HTMLInputElement).checked).toBe(false);
    await fireEvent.click(screen.getByTestId('review-tick-playbooks.press_enter'));
    await fireEvent.click(screen.getByTestId('review-apply-selected'));
    await waitFor(() => expect(decided).toEqual([{ accept: [1], reject: [] }]));
    await waitFor(() => expect(get(toasts).map((t) => t.message)).toContain('Proposed changes: applied 1'));
  });

  it('rejects what is ticked, and says which could not be decided', async () => {
    show('settings.review', [proposal({ id: 99 })]);
    await fireEvent.click(screen.getByTestId('review-apply-selected'));
    await waitFor(() =>
      expect(get(toasts).some((t) => t.kind === 'error' && t.message.includes('no longer waiting'))).toBe(true),
    );
    await fireEvent.click(screen.getByTestId('review-reject-selected'));
    await waitFor(() => expect(decided.at(-1)).toEqual({ accept: [], reject: [99] }));
  });

  it('opens a setting where it lives', async () => {
    const onopen = show('settings.review', [proposal()]);
    await fireEvent.click(screen.getByRole('button', { name: descs.get('work.recent_days')!.label }));
    expect(onopen).toHaveBeenCalledWith(homeOf(bundle.pages, 'work.recent_days')!.page, 'work.recent_days');
  });

  it('says when nothing waits, and offers no buttons read-only', () => {
    show('settings.review', []);
    expect(screen.getByTestId('review-empty')).toBeInTheDocument();
  });

  it('read-only (a paired desktop): the list without ticks or buttons', () => {
    show('settings.review', [proposal()], true);
    expect(screen.queryByTestId('review-tick-work.recent_days')).toBeNull();
    expect(screen.queryByTestId('review-apply-selected')).toBeNull();
  });
});

describe('a proposal inline, on its setting', () => {
  const home = () => homeOf(bundle.pages, 'work.recent_days')!.page;

  it('shows the suggestion beside the field, never as its value, and applies it on ✓', async () => {
    show(home(), [proposal()]);
    expect(screen.getByTestId('setting-suggestion-value-work.recent_days').textContent).toBe('3 days');
    expect((screen.getByTestId('setting-work-recent-days') as HTMLInputElement).value).toBe(defaults['work.recent_days']);
    expect(screen.queryByTestId('setting-suggestion-why-work.recent_days')).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: 'Why?' }));
    expect(screen.getByTestId('setting-suggestion-why-work.recent_days').textContent).toContain('shorter Recent list');
    await fireEvent.click(screen.getByTestId('setting-suggestion-apply-work.recent_days'));
    await waitFor(() => expect(decided).toEqual([{ accept: [1], reject: [] }]));
  });

  it('✗ rejects it', async () => {
    show(home(), [proposal()]);
    await fireEvent.click(screen.getByTestId('setting-suggestion-reject-work.recent_days'));
    await waitFor(() => expect(decided).toEqual([{ accept: [], reject: [1] }]));
  });

  it('History lists who changed it, from what to what', async () => {
    show(home(), []);
    await fireEvent.click(screen.getByTestId('setting-history-work.recent_days'));
    const list = await screen.findByTestId('setting-history-list-work.recent_days');
    await waitFor(() => expect(list.textContent).toContain('applying proposal #1'));
    expect(list.textContent).toMatch(/14 days\s*→\s*3 days · a person/);
    expect(list.textContent).toContain('an agent (control API)');
    expect(inv).toHaveBeenCalledWith('setting_history', { key: 'work.recent_days', limit: null });
  });

  it('read-only: no suggestion and no history', () => {
    show(home(), [proposal()], true);
    expect(screen.queryByTestId('setting-suggestion-work.recent_days')).toBeNull();
    expect(screen.queryByTestId('setting-history-work.recent_days')).toBeNull();
  });
});
