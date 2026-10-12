// Step 10.3: a settings change an agent proposed, decided from the chat. The
// card reads the proposal from the store, Apply asks first and writes once
// through `decide_setting_proposals`, and Not now writes nothing.
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import RichText from '../RichText.svelte';
import { settingsKey, settingsOpen, settingsSection } from '../app_views';
import { registryRouter } from '../pages/testing';
import { settingProposals, type SettingProposal } from '../pages/review';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const ui = (o: Record<string, unknown>) => '```fleet-ui\n' + JSON.stringify({ spec: 'fleet.ui/1', kind: 'setting', ...o }) + '\n```';

const proposal = (over: Partial<SettingProposal> = {}): SettingProposal => ({
  id: 1,
  at: 1_800_000_000,
  key: 'work.recent_days',
  value: '3',
  before: '14',
  current: '14',
  why: 'a shorter Recent list',
  source: 'agent',
  source_detail: 'mercury',
  state: 'pending',
  ...over,
});

let pending: SettingProposal[] = [];
let canWrite = true;
let decided: { accept: number[]; reject: number[] }[] = [];

beforeEach(() => {
  pending = [];
  canWrite = true;
  decided = [];
  settingProposals.set([]);
  settingsOpen.set(false);
  inv.mockReset();
  inv.mockImplementation(
    registryRouter({}, (cmd, args) => {
      if (cmd === 'setting_proposals') return { can_write: canWrite, proposals: pending };
      if (cmd === 'decide_setting_proposals') {
        const a = args as { accept: number[]; reject: number[] };
        decided.push(a);
        pending = pending.filter((p) => !a.accept.includes(p.id));
        return { applied: a.accept, rejected: a.reject, failed: [] };
      }
      return null;
    }).impl,
  );
});

const show = (o: Record<string, unknown>) => render(RichText, { props: { source: ui(o), sessionId: 1 } });

describe('setting card', () => {
  it('shows the change from the proposal, not from the block', async () => {
    pending = [proposal({ id: 11 })];
    show({ proposal: 11, note: 'Recent shows three days.' });
    const diff = await screen.findByTestId('rich-setting-diff');
    expect(diff.textContent).toContain('Recent work');
    expect(diff.textContent).toContain('− 14 days');
    expect(diff.textContent).toContain('+ 3 days');
    expect(screen.getByText('Recent shows three days.')).toBeTruthy();
    expect(screen.getByText(/suggested by an agent \(mercury\)/)).toBeTruthy();
    expect(screen.queryByTestId('rich-setting-moved')).toBeNull();
  });

  it('Apply asks first, then writes once', async () => {
    pending = [proposal({ id: 12 })];
    show({ proposal: 12 });
    await fireEvent.click(await screen.findByTestId('rich-setting-apply'));
    expect(decided).toEqual([]);
    const confirm = await screen.findByTestId('rich-setting-confirm');
    await fireEvent.click(confirm);
    await fireEvent.click(confirm);
    await waitFor(() => expect(screen.getByTestId('rich-setting-applied').textContent).toContain('Recent work is now 3 days'));
    expect(decided).toEqual([{ accept: [12], reject: [] }]);
  });

  it('Cancel in the confirm writes nothing', async () => {
    pending = [proposal({ id: 13 })];
    show({ proposal: 13 });
    await fireEvent.click(await screen.findByTestId('rich-setting-apply'));
    await fireEvent.click(await screen.findByText('Cancel'));
    expect(decided).toEqual([]);
    expect(screen.getByTestId('rich-setting-apply')).toBeTruthy();
  });

  it('Not now writes nothing and leaves the proposal waiting', async () => {
    pending = [proposal({ id: 14 })];
    show({ proposal: 14 });
    await fireEvent.click(await screen.findByTestId('rich-setting-later-btn'));
    expect(screen.getByTestId('rich-setting-later').textContent).toContain('Settings › Proposed changes');
    expect(decided).toEqual([]);
    expect(inv.mock.calls.map((c) => c[0])).not.toContain('decide_setting_proposals');
    expect(inv.mock.calls.map((c) => c[0])).not.toContain('set_fleet_setting');
  });

  it('a setting that warns shows its warning in the confirm', async () => {
    pending = [proposal({ id: 15, key: 'work.auto_tidy', value: 'true', before: 'false', current: 'false' })];
    show({ proposal: 15 });
    await fireEvent.click(await screen.findByTestId('rich-setting-apply'));
    expect(screen.getByTestId('confirm-dialog').textContent).toContain('safe-kill finished sessions');
  });

  it('says when the value moved since it was proposed', async () => {
    pending = [proposal({ id: 16, current: '30' })];
    show({ proposal: 16 });
    expect((await screen.findByTestId('rich-setting-moved')).textContent).toContain('it was 14 days');
    expect(screen.getByTestId('rich-setting-diff').textContent).toContain('− 30 days');
  });

  it('a proposal already decided offers Settings instead of Apply', async () => {
    show({ proposal: 17 });
    expect(await screen.findByTestId('rich-setting-gone')).toBeTruthy();
    expect(screen.queryByTestId('rich-setting-apply')).toBeNull();
  });

  it('a device that may not write settings has no Apply', async () => {
    canWrite = false;
    pending = [proposal({ id: 18 })];
    show({ proposal: 18 });
    expect(await screen.findByTestId('rich-setting-readonly')).toBeTruthy();
    expect(screen.queryByTestId('rich-setting-apply')).toBeNull();
  });

  it('Undo in Settings opens the setting where it lives', async () => {
    pending = [proposal({ id: 19 })];
    show({ proposal: 19 });
    await fireEvent.click(await screen.findByTestId('rich-setting-apply'));
    await fireEvent.click(await screen.findByTestId('rich-setting-confirm'));
    await fireEvent.click(await screen.findByTestId('rich-setting-open'));
    expect(get(settingsOpen)).toBe(true);
    expect(get(settingsSection)).toContain('.');
    expect(get(settingsKey)).toBe('work.recent_days');
  });
});
