// Layout L9 `guide` (declarative pages): a guide walks through a task one
// step at a time, and the Guides page reviews the guides an agent proposed.
import { render, screen, fireEvent, waitFor, within } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import PageView from './PageView.svelte';
import { allDescriptors, bundle } from './testing';
import {
  guideProposals,
  guidesError,
  guidesWritable,
  liveGuides,
  loadGuides,
  withheldGuides,
  type GuidesView,
} from './guides';
import { get } from 'svelte/store';
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
  withheldGuides.set([]);
  guideProposals.set([]);
  guidesWritable.set(true);
  guidesError.set(null);
});

describe('a guide, step by step', () => {
  it('leaves out a step this mode draws nothing for, and does not count it', () => {
    // `showData` hid every data item, but a page action and a custom component
    // are local-only too — so on a paired desktop a step of actions alone drew
    // an empty panel and still took a place in "Step N of M".
    const withButton: Page = {
      ...cleanup,
      sections: [
        (cleanup.sections ?? [])[0],
        { title: 'Run it', items: [{ type: 'action', action: bundle.actions[0].id }] },
      ],
    };
    const props = {
      page: withButton,
      pages: [...bundle.pages, withButton],
      descs,
      values: defaults,
      sources: bundle.sources,
      actions: bundle.actions,
      onnavigate: vi.fn(),
    };
    const local = render(PageView, { props });
    expect(screen.getByTestId('guide-progress').textContent).toBe('Step 1 of 2: What it does');
    local.unmount();

    render(PageView, { props: { ...props, remote: true } });
    expect(screen.getByTestId('guide-progress').textContent).toBe('Step 1 of 1: What it does');
  });


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
      props: {
        page: guidesPage,
        pages: bundle.pages,
        descs,
        values: defaults,
        sources: bundle.sources,
        actions: bundle.actions,
        onnavigate,
      },
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

  it('shows an approved guide the backend withholds, with its reason and a way out', async () => {
    // Such a row still holds a MAX_APPROVED slot, so leaving it off this screen
    // is what made an approved guide vanish with nothing saying why and nothing
    // able to remove it — the Remove button iterates the LIVE guides.
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'list_guides'
        ? {
            guides: [],
            proposals: [],
            can_write: true,
            withheld: [
              { id: 4, page_id: 'guide.orphan', why: 'item 2: links to `guide.cleanup`, which is not a page' },
            ],
          }
        : cmd === 'remove_guide'
          ? { guides: [], proposals: [], can_write: true, withheld: [] }
          : null,
    );
    await loadGuides();
    showReview();
    const row = screen.getByTestId('guide-held-guide.orphan');
    expect(row.textContent).toContain('guide.orphan');
    expect(row.textContent).toContain('which is not a page');

    // and it can be removed from here — no confirm, since it is not on the pages
    await fireEvent.click(screen.getByTestId('guide-remove-held-guide.orphan'));
    await waitFor(() => expect(screen.queryByTestId('guide-held-guide.orphan')).toBeNull());
  });

  it("names a step's button by the label it will carry, not by its action id", async () => {
    // The one item that DOES something was read out as its raw action id, so a
    // reviewer approving a button an agent put in a guide learned the least
    // about exactly that — and the confirm text it asks with, not at all.
    const act = bundle.actions[0];
    const withButton: Page = {
      ...cleanup,
      sections: [{ title: 'Run it', items: [{ type: 'action', action: act.id }] }],
    };
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'list_guides'
        ? { ...waiting, proposals: [{ ...waiting.proposals[0], page: withButton }] }
        : null,
    );
    await loadGuides();
    showReview();
    await fireEvent.click(screen.getByTestId('guide-preview-7'));
    const steps = screen.getByTestId('guide-steps-7');
    expect(steps.textContent).toContain(act.label);
    expect(steps.textContent).toContain(act.id);

    // An action this build does not have is said to be missing, not printed as
    // if it were a button.
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'list_guides'
        ? {
            ...waiting,
            proposals: [
              {
                ...waiting.proposals[0],
                page: { ...cleanup, sections: [{ title: 'Run it', items: [{ type: 'action', action: 'work.gone' }] }] },
              },
            ],
          }
        : null,
    );
    await loadGuides();
    showReview();
    await fireEvent.click(screen.getAllByTestId('guide-preview-7')[1]);
    expect(screen.getAllByTestId('guide-steps-7')[0].textContent).toContain('not an action of this build');
  });

  it("reads out a step's own prose and a stat's own label", async () => {
    // The preview is the only thing a person reads before making an
    // agent-authored guide live, and these two were the agent's free text: a
    // step's `intro` was never rendered at all, and a `stat` was read out as
    // fleet's source label with the guide's own words for the number dropped.
    const authored: Page = {
      ...cleanup,
      sections: [
        {
          title: 'What it does',
          intro: 'Trust me, this is completely safe and reversible.',
          items: [{ type: 'stat', source: { id: 'work.usage' }, label: 'Sessions we will stop' }],
        },
      ],
    };
    inv.mockImplementation(async (cmd: string) =>
      cmd === 'list_guides' ? { ...waiting, proposals: [{ ...waiting.proposals[0], page: authored }] } : null,
    );
    await loadGuides();
    showReview();
    await fireEvent.click(screen.getByTestId('guide-preview-7'));
    const steps = screen.getByTestId('guide-steps-7');
    expect(steps.textContent).toContain('completely safe and reversible');
    expect(steps.textContent).toContain('Sessions we will stop');
  });

  it("says the read failed rather than claiming the fleet has no guides", async () => {
    // `loadGuides`'s Result was discarded, so a failed read left the stores at
    // their initialisers and the page asserted "No guide is waiting" — the one
    // thing a failed call cannot establish — while `guidesWritable` stayed
    // `true`, which also hid the read-only note built for that case.
    inv.mockImplementation(async (cmd: string) => {
      if (cmd === 'list_guides') throw { code: 'E_HUB_PROTOCOL', message: 'this hub has no `guide` tool' };
      return null;
    });
    const r = await loadGuides();
    expect(r.ok).toBe(false);
    showReview();
    expect(screen.getByTestId('guide-review-error').textContent).toContain('no `guide` tool');
    expect(screen.queryByTestId('guide-review-empty')).toBeNull();
    // And this device no longer claims it may decide: `guidesWritable` stayed
    // at its `true` initialiser on a failed read, which is what drew an
    // Approve button for a device that had established nothing.
    expect(get(guidesWritable)).toBe(false);

    // And a read that works clears it.
    inv.mockImplementation(async (cmd: string) => (cmd === 'list_guides' ? waiting : null));
    await loadGuides();
    expect(get(guidesError)).toBeNull();
  });

  it('offers no decision to a device the hub does not trust', async () => {
    inv.mockImplementation(async (cmd: string) => (cmd === 'list_guides' ? { ...waiting, can_write: false } : null));
    await loadGuides();
    showReview();
    expect(screen.getByTestId('guide-review-readonly')).toBeTruthy();
    expect(screen.queryByTestId('guide-approve-7')).toBeNull();
  });
});
