// Layout L9 `guide` (declarative pages): a guide walks through a task one
// step at a time, and the Guides page reviews the guides an agent proposed.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import PageView from './PageView.svelte';
import { allDescriptors, bundle } from './testing';
import { guideApprovals, guideProposals, guideProvenance, guidesWritable, liveGuides, loadGuides, type GuidesView } from './guides';
import { guideChanges, guideSnapshot, guideSummary } from './guide_changes';
import type { Page } from './pages';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const descs = new Map(allDescriptors.map((d) => [d.key, d]));
const defaults = Object.fromEntries(allDescriptors.map((d) => [d.key, d.value]));

/** The shape `guide { catalog }` hands an author (service::guides::example). */
const cleanup: Page = {
  spec: 'fleet.page/1',
  id: 'guide.cleanup',
  title: 'Let fleet tidy up idle sessions',
  parent: 'guides',
  layout: 'guide',
  intro: 'Three steps: what cleanup does, turn it on, and when it acts.',
  sections: [
    {
      title: 'What it does',
      items: [{ type: 'notice', tone: 'info', text: 'Garbage collection stops background sessions that sat idle.' }],
    },
    { title: 'Turn it on', items: [{ type: 'field', key: 'gc.enabled' }] },
    {
      title: 'When it acts',
      when: { key: 'gc.enabled', truthy: true },
      items: [
        { type: 'field', key: 'gc.bg_idle_secs', hint: 'A day suits most fleets.' },
        { type: 'link', page: 'settings.automation', label: 'Every automation setting' },
      ],
    },
  ],
};

function showGuide(values: Record<string, string>) {
  const onnavigate = vi.fn();
  render(PageView, {
    props: {
      page: cleanup,
      pages: [...bundle.pages, cleanup],
      descs,
      values: { ...defaults, ...values },
      sources: bundle.sources,
      onnavigate,
    },
  });
  return onnavigate;
}

beforeEach(() => {
  inv.mockReset();
  liveGuides.set([]);
  guideProposals.set([]);
  guidesWritable.set(true);
  guideApprovals.set(new Map());
});

describe('a guide, step by step', () => {
  it('shows one step at a time with Back, Next and Done', async () => {
    const onnavigate = showGuide({ 'gc.enabled': 'true' });
    expect(screen.getByTestId('guide-progress').textContent).toBe('Step 1 of 3: What it does');
    expect(screen.queryByTestId('setting-row-gc.enabled')).toBeNull();
    expect((screen.getByTestId('guide-back') as HTMLButtonElement).disabled).toBe(true);

    await fireEvent.click(screen.getByTestId('guide-next'));
    expect(screen.getByTestId('guide-progress').textContent).toBe('Step 2 of 3: Turn it on');
    // The registry gives the field its label: the guide only names the key.
    expect(screen.getByTestId('setting-row-gc.enabled').textContent).toContain(descs.get('gc.enabled')!.label);

    await fireEvent.click(screen.getByTestId('guide-next'));
    expect(screen.getByTestId('setting-row-gc.bg_idle_secs').textContent).toContain('A day suits most fleets.');
    expect(screen.queryByTestId('guide-next')).toBeNull();
    await fireEvent.click(screen.getByTestId('guide-done'));
    expect(onnavigate).toHaveBeenCalledWith('guides');

    await fireEvent.click(screen.getByTestId('guide-step-0'));
    expect(screen.getByTestId('guide-progress').textContent).toBe('Step 1 of 3: What it does');
  });

  it("drops a step whose condition does not hold, so the count follows the answers", async () => {
    showGuide({ 'gc.enabled': 'false' });
    expect(screen.getByTestId('guide-progress').textContent).toBe('Step 1 of 2: What it does');
    expect(within(screen.getByTestId('guide-steps')).queryByText('When it acts')).toBeNull();
    await fireEvent.click(screen.getByTestId('guide-next'));
    expect(screen.getByTestId('guide-done')).toBeTruthy();
  });
});

describe('where a guide came from and what it changed (G7.15)', () => {
  it('says who proposed a live guide and who approved it, and when', () => {
    const at = Date.UTC(2026, 9, 6, 12) / 1000;
    expect(
      guideProvenance({ page_id: 'guide.cleanup', source: 'agent', source_detail: 'host web-1', approved_by: 'person (pixel)', approved_at: at }, 'UTC'),
    ).toBe('Guide · proposed by an agent (host web-1), approved by a person (pixel) on 6 Oct');
    expect(guideProvenance(undefined)).toBe('Guide · comes with Fleet');
  });

  it('heads the guide with that line', () => {
    guideApprovals.set(new Map([['guide.cleanup', { page_id: 'guide.cleanup', source: 'agent', approved_by: 'person' }]]));
    showGuide({});
    expect(screen.getByTestId('guide-provenance').textContent).toBe('Guide · proposed by an agent, approved by a person');
  });

  it('counts the steps and the settings a guide changes', () => {
    expect(guideSummary(cleanup, descs)).toBe('3 steps · changes 2 settings');
  });

  it('lists each setting changed since the guide opened, and Undo writes the old value back', async () => {
    const before = guideSnapshot(cleanup, descs, { ...defaults, 'gc.enabled': 'false' });
    expect(guideChanges(cleanup, descs, before, { ...defaults, 'gc.enabled': 'false' })).toEqual([]);
    const [c] = guideChanges(cleanup, descs, before, { ...defaults, 'gc.enabled': 'true' });
    expect([c.key, c.before, c.words]).toEqual(['gc.enabled', 'false', 'Off → On']);

    inv.mockResolvedValue({});
    const { rerender } = render(PageView, {
      props: { page: cleanup, pages: [...bundle.pages, cleanup], descs, values: { ...defaults, 'gc.enabled': 'false' }, sources: bundle.sources, onnavigate: vi.fn() },
    });
    expect(screen.queryByTestId('guide-changes')).toBeNull();
    await rerender({ values: { ...defaults, 'gc.enabled': 'true' } });
    expect(screen.getByTestId('guide-change-gc.enabled').textContent).toContain('Off → On');
    await fireEvent.click(screen.getByTestId('guide-change-undo-gc.enabled'));
    await waitFor(() => expect(inv).toHaveBeenCalledWith('set_fleet_setting', { key: 'gc.enabled', value: 'false' }));
  });
});

describe('the Guides page', () => {
  const guidesPage = bundle.pages.find((p) => p.id === 'guides') as Page;
  const waiting: GuidesView = {
    guides: [],
    proposals: [
      {
        id: 7,
        at: Math.floor(Date.now() / 1000) - 60,
        page_id: 'guide.cleanup',
        title: cleanup.title,
        why: 'People keep asking how cleanup works.',
        source: 'agent',
        source_detail: 'host web-1',
        replaces: false,
        page: cleanup,
      },
    ],
    can_write: true,
  };

  function showReview() {
    const onnavigate = vi.fn();
    render(PageView, {
      props: { page: guidesPage, pages: bundle.pages, descs, values: defaults, sources: bundle.sources, onnavigate },
    });
    return onnavigate;
  }

  it('is compiled in as a review of guides', () => {
    expect(guidesPage.layout).toBe('review_apply');
    expect(guidesPage.review).toBe('guides');
  });

  it("reads a proposal's steps in words, then approves it onto the pages", async () => {
    inv.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'list_guides') return waiting;
      if (cmd === 'decide_guide') {
        expect(args).toEqual({ id: 7, approve: true });
        return { guides: [cleanup], proposals: [], can_write: true };
      }
      if (cmd === 'remove_guide') {
        expect(args).toEqual({ pageId: 'guide.cleanup' });
        return { guides: [], proposals: [], can_write: true };
      }
      return null;
    });
    await loadGuides();
    const onnavigate = showReview();
    const row = screen.getByTestId('guide-proposal-7');
    expect(row.textContent).toContain('proposed by an agent (host web-1)');
    expect(row.textContent).toContain('People keep asking');

    await fireEvent.click(screen.getByTestId('guide-preview-7'));
    const steps = screen.getByTestId('guide-steps-7').textContent ?? '';
    expect(steps).toContain(`Setting: ${descs.get('gc.enabled')!.label} (gc.enabled)`);
    expect(steps).toContain('Link: Every automation setting');
    expect(steps).toContain('(only when it applies)');

    await fireEvent.click(screen.getByTestId('guide-approve-7'));
    await waitFor(() => expect(screen.getByTestId('guide-live-guide.cleanup')).toBeTruthy());
    expect(screen.getByTestId('guide-review-empty')).toBeTruthy();
    await fireEvent.click(within(screen.getByTestId('guide-live-guide.cleanup')).getByText(cleanup.title));
    expect(onnavigate).toHaveBeenCalledWith('guide.cleanup');

    await fireEvent.click(screen.getByTestId('guide-remove-guide.cleanup'));
    await fireEvent.click(screen.getByTestId('guide-remove-confirm'));
    await waitFor(() => expect(screen.queryByTestId('guide-live-guide.cleanup')).toBeNull());
  });

  it('offers no decision to a device the hub does not trust', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'list_guides' ? { ...waiting, can_write: false } : null));
    await loadGuides();
    showReview();
    expect(screen.getByTestId('guide-review-readonly')).toBeTruthy();
    expect(screen.queryByTestId('guide-approve-7')).toBeNull();
  });
});
