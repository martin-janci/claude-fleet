import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import AssetsRail from './AssetsRail.svelte';

const props = (over: Record<string, unknown> = {}) => ({
  view: 'inbox' as const,
  counts: { inbox: 3, layers: 4, hosts: 5, library: 12 },
  readOnly: false,
  onview: vi.fn(),
  onsecrets: vi.fn(),
  ...over,
});

describe('AssetsRail', () => {
  it('has Inbox, Layers, Hosts, Library and Secrets, in that order', () => {
    render(AssetsRail, props());
    const labels = Array.from(document.querySelectorAll('nav button')).map((b) => b.textContent?.replace(/\d+/g, '').trim());
    expect(labels).toEqual(['Inbox', 'Layers', 'Hosts', 'Library', 'Secrets']);
  });

  it('marks the current view in words (aria-current), not by colour alone, and shows the counts', () => {
    render(AssetsRail, props({ view: 'library' }));
    expect(screen.getByTestId('assets-rail-library').getAttribute('aria-current')).toBe('page');
    expect(screen.getByTestId('assets-rail-inbox').getAttribute('aria-current')).toBeNull();
    expect(screen.getByTestId('assets-rail-inbox').textContent).toContain('3');
    expect(screen.getByTestId('assets-rail-library').textContent).toContain('12');
  });

  it('a zero count is not drawn', () => {
    render(AssetsRail, props({ counts: { inbox: 0, layers: 0, hosts: 0, library: 2 } }));
    expect(screen.getByTestId('assets-rail-inbox').textContent?.trim()).toBe('Inbox');
  });

  it('tells the workspace which view and opens Secrets', async () => {
    const p = props();
    render(AssetsRail, p);
    await fireEvent.click(screen.getByTestId('assets-rail-library'));
    expect(p.onview).toHaveBeenCalledWith('library');
    await fireEvent.click(screen.getByTestId('assets-secrets'));
    expect(p.onsecrets).toHaveBeenCalled();
  });

  it('Secrets is disabled while something runs, as the old toolbar button was', () => {
    render(AssetsRail, props({ busy: true }));
    expect(screen.getByTestId('assets-secrets')).toBeDisabled();
    expect(screen.getByTestId('assets-rail-library')).not.toBeDisabled();
  });
  it('read-only: no Secrets', () => {
    render(AssetsRail, props({ readOnly: true }));
    expect(screen.queryByTestId('assets-secrets')).toBeNull();
    expect(screen.getByTestId('assets-rail-library')).toBeTruthy();
  });
});
