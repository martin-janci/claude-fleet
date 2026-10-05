import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, it, expect, vi } from 'vitest';
import CatalogChip from './CatalogChip.svelte';

const repo = { head: 'abcdef1234567890', dirty: 2, ahead: 3, behind: 0, has_upstream: true };

describe('CatalogChip', () => {
  it('shows HEAD, ahead and dirty on the chip', () => {
    render(CatalogChip, { name: 'personal', head: null, repo });
    expect(screen.getByTestId('assets-head').textContent).toBe('personal @abcdef1 ↑3 ±2');
  });
  it('opens a popover with pull, commit and push for a catalog it may write', async () => {
    const onpush = vi.fn();
    const oncommit = vi.fn();
    const onpull = vi.fn();
    render(CatalogChip, { name: 'personal', head: null, repo, writable: true, onpush, oncommit, onpull });
    const chip = screen.getByTestId('catalog-chip-personal');
    expect(chip.getAttribute('aria-expanded')).toBe('false');
    await fireEvent.click(chip);
    expect(chip.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByTestId('assets-repo-status').textContent).toContain('2 dirty');
    // PF2: pressing an action closes the popover, so each is reached by re-opening it.
    expect(screen.getByTestId('assets-push').textContent).toContain('↑3');
    await fireEvent.click(screen.getByTestId('assets-push'));
    expect(onpush).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('dialog')).toBeNull();
    await fireEvent.click(chip);
    await fireEvent.click(screen.getByTestId('assets-commit-pending'));
    expect(oncommit).toHaveBeenCalledTimes(1);
    await fireEvent.click(chip);
    await fireEvent.click(screen.getByTestId('assets-pull'));
    expect(onpull).toHaveBeenCalledTimes(1);
  });
  it('disables push without an upstream and hides commit when clean', async () => {
    render(CatalogChip, { name: 'personal', head: null, repo: { ...repo, dirty: 0, has_upstream: false, ahead: null }, writable: true, onpush: vi.fn() });
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    expect(screen.getByTestId('assets-push')).toBeDisabled();
    expect(screen.queryByTestId('assets-commit-pending')).toBeNull();
  });
  it('disables the actions while something else is running', async () => {
    render(CatalogChip, { name: 'personal', head: null, repo, writable: true, busy: true });
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    expect(screen.getByTestId('assets-pull')).toBeDisabled();
    expect(screen.getByTestId('assets-push')).toBeDisabled();
    expect(screen.getByTestId('assets-commit-pending')).toBeDisabled();
  });
  it('an org catalog: status only, and how it gets pushed', async () => {
    render(CatalogChip, { name: 'papayapos', head: '9f0e1d2aaaa', state: 'problem', problem: 'not a git repository' });
    const chip = screen.getByTestId('catalog-chip-papayapos');
    expect(chip.textContent).toContain('papayapos @9f0e1d2 ⚠');
    expect(chip.getAttribute('title')).toBe('not a git repository');
    await fireEvent.click(chip);
    expect(screen.queryByTestId('assets-push')).toBeNull();
    expect(screen.queryByTestId('assets-pull')).toBeNull();
    const dialog = screen.getByRole('dialog');
    expect(dialog.textContent).toContain('catalog.auto_push');
    expect(dialog.textContent).toContain('not a git repository');
  });
  it('an org catalog that failed to load names the failure on the chip, and one not loaded says so', async () => {
    render(CatalogChip, { name: 'acme', head: null, state: 'not_loaded' });
    const chip = screen.getByTestId('catalog-chip-acme');
    expect(chip.textContent).toContain('acme @—');
    await fireEvent.click(chip);
    expect(screen.getByRole('dialog').textContent).toContain('not loaded');
  });
  it('an org catalog shows its ahead and dirty counts once they are read', () => {
    render(CatalogChip, { name: 'papayapos', head: '9f0e1d2aaaa', repo: { ...repo, head: '9f0e1d2aaaa', dirty: 0, ahead: 1 } });
    expect(screen.getByTestId('catalog-chip-papayapos').textContent).toContain('papayapos @9f0e1d2 ↑1');
  });
  it('a read-only caller sees the status but no pull, commit or push', async () => {
    render(CatalogChip, { name: 'personal', head: null, repo, writable: false });
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    expect(screen.getByTestId('assets-repo-status').textContent).toContain('2 dirty');
    for (const id of ['assets-pull', 'assets-commit-pending', 'assets-push']) expect(screen.queryByTestId(id)).toBeNull();
  });
  it('Esc closes the popover and the focus goes back to the chip', async () => {
    render(CatalogChip, { name: 'personal', head: 'abc', repo, writable: true });
    const chip = screen.getByTestId('catalog-chip-personal');
    await fireEvent.click(chip);
    await fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.activeElement).toBe(chip);
  });
  it('moves the focus into the popover when it opens', async () => {
    render(CatalogChip, { name: 'personal', head: 'abc', repo, writable: true });
    await fireEvent.click(screen.getByTestId('catalog-chip-personal'));
    expect(screen.getByRole('dialog').contains(document.activeElement)).toBe(true);
  });
});
