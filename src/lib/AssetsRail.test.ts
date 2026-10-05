import { render, screen, fireEvent } from '@testing-library/svelte';
import { readFileSync } from 'node:fs';
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

  // The labels hide below 1100 px (the rail is an icon strip), so each button
  // names itself by aria-label and title, with its count in words.
  it('every button keeps its name when the label text hides: aria-label and title', () => {
    render(AssetsRail, props());
    expect(screen.getByTestId('assets-rail-inbox').getAttribute('aria-label')).toBe('Inbox, 3');
    expect(screen.getByTestId('assets-rail-inbox').getAttribute('title')).toBe('Inbox');
    expect(screen.getByTestId('assets-secrets').getAttribute('aria-label')).toBe('Secrets');
    expect(screen.getByRole('button', { name: 'Hosts, 5' })).toBeTruthy();
  });

  it('a zero count is not in the name', () => {
    render(AssetsRail, props({ counts: { inbox: 0, layers: 0, hosts: 0, library: 2 } }));
    expect(screen.getByTestId('assets-rail-inbox').getAttribute('aria-label')).toBe('Inbox');
  });

  it('hides the labels (not the buttons) below 1100px', () => {
    const css = readFileSync('src/lib/AssetsRail.svelte', 'utf8').match(/<style[^>]*>([\s\S]*?)<\/style>/)?.[1] ?? '';
    const block = /@media \(max-width: 1100px\)\s*\{([\s\S]*?)\n  \}/.exec(css)?.[1] ?? '';
    expect(block).toMatch(/\.lbl\s*\{\s*display:\s*none/);
  });
});
