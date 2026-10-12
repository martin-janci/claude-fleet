// Redesign 9.11: LLM drafts in Control. The morning brief is drafted only on
// Refresh (opening Today reads the last one), and a completed mission's
// release note is drafted on demand, then edited and copied.
import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { tick } from 'svelte';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('./clipboard', () => ({ copyText: vi.fn(async () => true) }));
import { invoke } from '@tauri-apps/api/core';
import { copyText } from './clipboard';
import MorningBrief from './MorningBrief.svelte';
import ReleaseNote from './ReleaseNote.svelte';
import { draftedAt, type Draft } from './drafts';
import { fleetSettings, SETTING_DEFAULTS } from './fleet_settings';

const draft: Draft = {
  text: 'PAY-7 waits on you.',
  model: 'haiku',
  host_alias: 'mercury',
  from: '3 items and 1 shipped',
  at: new Date(2026, 9, 8, 8, 2).getTime() / 1000,
};

/** `refresh` of a `today_brief` call; `undefined` for any other command. */
function refreshOf(cmd: string, a: unknown): boolean | undefined {
  return cmd === 'today_brief' ? (a as { args: { refresh: boolean } }).args.refresh : undefined;
}

async function flush() {
  for (let i = 0; i < 5; i++) await tick();
}

function briefCalls(): { refresh: boolean }[] {
  return vi
    .mocked(invoke)
    .mock.calls.filter((c) => c[0] === 'today_brief')
    .map((c) => (c[1] as { args: { refresh: boolean } }).args);
}

describe('MorningBrief', () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it('reads the brief drafted last on open and never drafts one', async () => {
    vi.mocked(invoke).mockImplementation(async (c) => (c === 'today_brief' ? {} : null));
    render(MorningBrief);
    await flush();
    expect(briefCalls()).toEqual([expect.objectContaining({ refresh: false })]);
    expect(screen.getByTestId('morning-brief-draft-btn')).toBeTruthy();
    expect(screen.queryByTestId('morning-brief-draft')).toBeNull();
  });

  it('shows the last brief with its time, and Refresh drafts a new one', async () => {
    vi.mocked(invoke).mockImplementation(async (c, a) =>
      refreshOf(c, a) ? { draft: { ...draft, text: 'New.' } } : { draft },
    );
    render(MorningBrief);
    await flush();
    expect(screen.getByTestId('morning-brief-at').textContent).toBe('Drafted 08:02');
    const input = screen.getByTestId('morning-brief-draft-input') as HTMLTextAreaElement;
    expect(input.value).toBe('PAY-7 waits on you.');
    expect(screen.getByTestId('morning-brief-draft-meta').textContent).toContain(
      'by haiku on mercury · from 3 items and 1 shipped',
    );
    expect(briefCalls()).toHaveLength(1);
    await fireEvent.click(screen.getByTestId('morning-brief-draft-regenerate'));
    await flush();
    expect(briefCalls().map((a) => a.refresh)).toEqual([false, true]);
    expect((screen.getByTestId('morning-brief-draft-input') as HTMLTextAreaElement).value).toBe('New.');
  });

  it('stays away on a hub older than the brief', async () => {
    vi.mocked(invoke).mockImplementation(async (c) => {
      if (c === 'today_brief') throw { code: 'E_HUB_PROTOCOL', message: 'no such action' };
      return null;
    });
    render(MorningBrief);
    await flush();
    expect(screen.queryByTestId('morning-brief')).toBeNull();
  });

  it('says a refusal in its own words, never hiding as an older hub would', async () => {
    vi.mocked(invoke).mockImplementation(async (c) => {
      if (c === 'today_brief') throw { code: 'E_FORBIDDEN', message: 'your org has not consented to LLM drafts' };
      return null;
    });
    render(MorningBrief);
    await flush();
    expect(screen.getByTestId('morning-brief')).toBeTruthy();
    expect(screen.getByTestId('morning-brief-refused').textContent).toBe('your org has not consented to LLM drafts');
    expect(screen.queryByTestId('morning-brief-draft-btn')).toBeNull();
  });

  it('says why a refresh failed', async () => {
    vi.mocked(invoke).mockImplementation(async (c, a) => {
      if (refreshOf(c, a)) {
        throw { code: 'E_INVALID_STATE', message: 'no session is running today' };
      }
      return {};
    });
    render(MorningBrief);
    await flush();
    await fireEvent.click(screen.getByTestId('morning-brief-draft-btn'));
    await flush();
    expect(screen.getByTestId('morning-brief-error').textContent).toContain('no session is running today');
  });
});

describe('ReleaseNote', () => {
  beforeEach(() => {
    fleetSettings.set({ ...SETTING_DEFAULTS, 'work.draft_release_notes': 'true' });
    vi.mocked(invoke).mockReset();
    vi.mocked(copyText).mockClear();
  });

  it('drafts only when asked, then copies the edited text', async () => {
    vi.mocked(invoke).mockImplementation(async (c) =>
      c === 'mission_release_note' ? { ...draft, text: 'Login is passwordless.', from: '2 tasks' } : null,
    );
    render(ReleaseNote, { missionId: 4 });
    await flush();
    expect(vi.mocked(invoke).mock.calls.some((c) => c[0] === 'mission_release_note')).toBe(false);
    await fireEvent.click(screen.getByTestId('release-note-draft-btn'));
    await flush();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('mission_release_note', { args: { mission_id: 4 } });
    const input = screen.getByTestId('release-note-draft-input') as HTMLTextAreaElement;
    expect(input.value).toBe('Login is passwordless.');
    await fireEvent.input(input, { target: { value: 'Login is passwordless now.' } });
    await fireEvent.click(screen.getByTestId('release-note-copy'));
    expect(copyText).toHaveBeenCalledWith('Login is passwordless now.');
  });

  it('says why a draft failed', async () => {
    vi.mocked(invoke).mockImplementation(async (c) => {
      if (c === 'mission_release_note') throw { code: 'E_INVALID_STATE', message: 'no host for the planner' };
      return null;
    });
    render(ReleaseNote, { missionId: 4 });
    await fireEvent.click(screen.getByTestId('release-note-draft-btn'));
    await flush();
    expect(screen.getByTestId('release-note-error').textContent).toContain('no host for the planner');
  });
});

describe('draftedAt', () => {
  it('is the local time, zero-padded', () => {
    expect(draftedAt(new Date(2026, 0, 1, 7, 5).getTime() / 1000)).toBe('Drafted 07:05');
  });
});
