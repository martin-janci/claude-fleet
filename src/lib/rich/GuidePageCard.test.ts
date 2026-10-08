// Step 10.4: a guide fleet already has opens as a card in the chat, drawn by
// the same PageView as Settings › Guides, so it is the same guide in both
// places: the same steps, the same live fields, the same Done.
import { render, screen, fireEvent, within, cleanup as unmount } from '@testing-library/svelte';
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

import { invoke as mockedInvoke } from '@tauri-apps/api/core';
import RichText from '../RichText.svelte';
import PageView from '../pages/PageView.svelte';
import { settingsOpen, settingsSection } from '../app_views';
import { liveGuides } from '../pages/guides';
import { allDescriptors, bundle, registryRouter } from '../pages/testing';
import type { Page } from '../pages/pages';

const inv = mockedInvoke as ReturnType<typeof vi.fn>;
const descs = new Map(allDescriptors.map((d) => [d.key, d]));
const defaults = Object.fromEntries(allDescriptors.map((d) => [d.key, d.value]));

const cleanup: Page = {
  spec: 'fleet.page/1',
  id: 'guide.cleanup',
  title: 'Let fleet tidy up idle sessions',
  parent: 'guides',
  layout: 'guide',
  intro: 'Three steps: what cleanup does, turn it on, and when it acts.',
  sections: [
    { title: 'What it does', items: [{ type: 'notice', tone: 'info', text: 'Garbage collection stops background sessions that sat idle.' }] },
    { title: 'Turn it on', items: [{ type: 'field', key: 'gc.enabled' }] },
    { title: 'When it acts', items: [{ type: 'field', key: 'gc.bg_idle_secs' }, { type: 'link', page: 'settings.automation', label: 'Every automation setting' }] },
  ],
} as Page;

const ui = (o: Record<string, unknown>) => '```fleet-ui\n' + JSON.stringify({ spec: 'fleet.ui/1', kind: 'guide', ...o }) + '\n```';
const inChat = (page: string) => render(RichText, { props: { source: ui({ page }), sessionId: 1 } });

beforeEach(() => {
  liveGuides.set([]);
  settingsOpen.set(false);
  settingsSection.set(null);
  inv.mockReset();
  inv.mockImplementation(
    registryRouter({}, (cmd) => (cmd === 'list_guides' ? { guides: [cleanup], proposals: [], can_write: true } : null)).impl,
  );
});

/** What a person reads of a guide: its title, the step list and the step shown. */
function read(root: HTMLElement) {
  return {
    title: root.querySelector('h4')?.textContent,
    steps: within(root).getAllByRole('listitem').map((li) => li.textContent?.trim()),
    now: within(root).getByTestId('guide-progress').textContent,
  };
}

describe('a guide in the chat', () => {
  it('is the same guide Settings shows', async () => {
    inChat('guide.cleanup');
    await screen.findByTestId('guide-steps');
    const chat = read(screen.getByTestId('rich-guide-page'));
    unmount();
    render(PageView, {
      props: { page: cleanup, pages: [...bundle.pages, cleanup], descs, values: defaults, sources: bundle.sources, onnavigate: () => {} },
    });
    expect(chat).toEqual(read(document.body));
    expect(chat.title).toBe('Let fleet tidy up idle sessions');
  });

  it('walks the steps with live fields, and Done folds the card', async () => {
    inChat('guide.cleanup');
    await screen.findByTestId('guide-steps');
    await fireEvent.click(screen.getByTestId('guide-next'));
    expect(screen.getByTestId('setting-row-gc.enabled').textContent).toContain(descs.get('gc.enabled')!.label);
    await fireEvent.click(screen.getByTestId('guide-next'));
    await fireEvent.click(screen.getByTestId('guide-done'));
    expect(screen.getByTestId('rich-guide-page-done').textContent).toContain('Let fleet tidy up idle sessions');
    expect(get(settingsOpen)).toBe(false);
  });

  it('Open in Settings opens the same guide there', async () => {
    inChat('guide.cleanup');
    await fireEvent.click(await screen.findByTestId('rich-guide-page-open'));
    expect(get(settingsOpen)).toBe(true);
    expect(get(settingsSection)).toBe('guide.cleanup');
  });

  it('a guide this fleet does not have says so', async () => {
    inChat('guide.nope');
    expect((await screen.findByTestId('rich-guide-page-missing')).textContent).toContain('guide.nope');
  });

  it('an inline guide is still drawn from the block', () => {
    render(RichText, { props: { source: ui({ title: 'Missions', sections: [{ title: 'What', body: 'A mission' }] }), sessionId: 1 } });
    expect(screen.getByTestId('rich-guide')).toBeTruthy();
    expect(screen.queryByTestId('rich-guide-page')).toBeNull();
  });
});
